use anyhow::{Result, Context};
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::time::{timeout, Duration};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use sha2::{Sha256, Digest};

/// Process-wide counter used to build collision-free temp file names.
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Build a unique temp file path so concurrent compilations never clash on the
/// same source/output file (R3 makes concurrent compiles possible).
pub(crate) fn unique_temp_file(prefix: &str, extension: &str) -> PathBuf {
    let id = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("{}_{}_{}.{}", prefix, std::process::id(), id, extension))
}

/// Build a unique temp directory path (per-request working directory, F7).
pub(crate) fn unique_temp_dir(prefix: &str) -> PathBuf {
    let id = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("{}_{}_{}", prefix, std::process::id(), id))
}

/// Cap for a child's stderr. Smaller than the large text cap because stderr is
/// only used for diagnostics (and, for `-dump-tokens`, token listings).
pub(crate) const CHILD_STDERR_CAP: usize = 256 * 1024; // 256 KiB

/// Cap for large text outputs (LLVM IR, assembly, objdump disassembly).
pub(crate) const CHILD_TEXT_CAP: usize = 2 * 1024 * 1024; // 2 MiB

/// Bounded child output (stdout/stderr) plus the exit status.
pub(crate) struct CappedOutput {
    pub status: std::process::ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    /// True when at least one stream hit its cap and was cut short.
    pub truncated: bool,
}

/// Read a stream until EOF or until `cap` bytes have been buffered.
///
/// Takes ownership of the reader so the pipe is closed as soon as the cap is
/// reached (which unblocks/kills a child that keeps writing).
async fn read_capped<R>(mut reader: R, cap: usize) -> std::io::Result<(Vec<u8>, bool)>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut buf = Vec::with_capacity(cap.min(64 * 1024));
    let mut chunk = [0u8; 8192];

    loop {
        let n = reader.read(&mut chunk).await?;
        if n == 0 {
            return Ok((buf, false));
        }

        if buf.len() + n > cap {
            let remaining = cap.saturating_sub(buf.len());
            buf.extend_from_slice(&chunk[..remaining]);
            return Ok((buf, true));
        }

        buf.extend_from_slice(&chunk[..n]);
    }
}

/// Spawn `cmd`, capture stdout/stderr with explicit caps, and wait for it.
///
/// This bounds how much child output the Rust process buffers: once a stream
/// reaches its cap the reader stops (closing the pipe) and the child is killed,
/// so a runaway clang/objdump cannot grow our heap without bound. `kill_on_drop`
/// is enabled so a caller-side `timeout` also reaps the process.
pub(crate) async fn run_capped(
    cmd: &mut Command,
    stdout_cap: usize,
    stderr_cap: usize,
) -> Result<CappedOutput> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = cmd.spawn().context("Failed to spawn child process")?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("child stdout pipe unavailable"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow::anyhow!("child stderr pipe unavailable"))?;

    // Drain both pipes concurrently. Each reader owns its pipe and drops it on
    // return, so hitting a cap closes that stream immediately.
    let (out_res, err_res) = tokio::join!(
        read_capped(stdout, stdout_cap),
        read_capped(stderr, stderr_cap),
    );

    let (stdout, stdout_truncated) = out_res.context("Failed to read child stdout")?;
    let (stderr, stderr_truncated) = err_res.context("Failed to read child stderr")?;

    let truncated = stdout_truncated || stderr_truncated;
    if truncated {
        // We already have all the output we are willing to keep; stop the child.
        let _ = child.kill().await;
    }

    let status = child.wait().await.context("Failed to wait for child process")?;

    Ok(CappedOutput {
        status,
        stdout,
        stderr,
        truncated,
    })
}

pub struct CompilerOrchestrator {
    cache: Mutex<CompilationCache>,
    /// `None` when no compiler was found at startup. The server still boots in
    /// that state (R11); `compile()` then returns a clear error.
    compiler_path: Option<PathBuf>,
}

impl CompilerOrchestrator {
    /// Create the orchestrator.
    ///
    /// Never fails: if clang cannot be found the instance is created in an
    /// "unavailable" state so the process can still start (R11).
    pub fn new() -> Self {
        let compiler_path = Self::find_compiler();

        if compiler_path.is_none() {
            log::warn!(
                "Clang not found. /api/compile will return 503 until a compiler is installed."
            );
        }

        Self {
            cache: Mutex::new(CompilationCache::new(1000)), // Cache 1000 compilations
            compiler_path,
        }
    }

    /// Whether a usable compiler was discovered at startup.
    pub fn is_available(&self) -> bool {
        self.compiler_path.is_some()
    }

    /// The resolved compiler binary, if one was found at startup.
    ///
    /// Used by `/api/run`, which links a native executable (same resolution as
    /// `/api/compile`).
    pub fn compiler_path(&self) -> Option<&Path> {
        self.compiler_path.as_deref()
    }

    fn find_compiler() -> Option<PathBuf> {
        let possible_paths = [
            "./compilers/clang",                     // Bundled compiler
            "/usr/bin/clang",                        // Linux
            "C:\\msys64\\mingw64\\bin\\clang.exe",   // Windows
            "clang",                                 // PATH
        ];

        for path in possible_paths {
            if Path::new(path).exists() {
                log::info!("Found compiler at: {}", path);
                return Some(PathBuf::from(path));
            }
        }

        None
    }

    pub async fn compile(&self, request: CompilationRequest) -> Result<CompilationResult> {
        // Keep the platform gate compile-time cheap but runtime-correct: Windows
        // is unsupported and returns the same clear message as before.
        if cfg!(not(unix)) {
            return Err(anyhow::anyhow!(
                "Windows is not currently supported. \
                Pipeline requires a Unix-like system (Linux, macOS) with clang. \
                \n\nWorkarounds: \
                \n1. Use WSL2 (Windows Subsystem for Linux) \
                \n2. Use Docker: docker run -v %cd%:/app -p 8080:8080 pipeline-backend \
                \n3. Use a Linux VM or remote server"
            ));
        }

        let compiler_path = self.compiler_path.as_ref().ok_or_else(|| {
            anyhow::anyhow!("No C compiler (clang) is available on this server")
        })?;

        let cache_key = self.generate_cache_key(&request);

        // Check cache
        if let Some(cached) = self.cache.lock().unwrap().get(&cache_key) {
            log::info!("Cache hit for compilation");
            return Ok(cached);
        }

        log::info!("Cache miss, compiling...");

        let result = self.compile_in_sandbox(compiler_path, &request).await?;

        // Cache result
        self.cache.lock().unwrap().insert(cache_key, result.clone());

        Ok(result)
    }

    fn generate_cache_key(&self, request: &CompilationRequest) -> CacheKey {
        let mut hasher = Sha256::new();
        hasher.update(request.source_code.as_bytes());
        hasher.update(request.compiler.as_bytes());
        for flag in &request.flags {
            hasher.update(flag.as_bytes());
        }
        let hash = format!("{:x}", hasher.finalize());

        CacheKey {
            hash,
            compiler: request.compiler.clone(),
        }
    }

    async fn compile_in_sandbox(
        &self,
        compiler_path: &Path,
        request: &CompilationRequest,
    ) -> Result<CompilationResult> {
        // Resource limits (R6): we deliberately do NOT apply RLIMIT_AS /
        // `ulimit -v` to clang -- it reserves a very large virtual address space
        // and crashes under an address-space cap. Memory is bounded instead by:
        //   * the shared `MAX_CONCURRENT_COMPILES` semaphore (bounded parallelism),
        //   * the container `--memory` limit (see Dockerfile / DEPLOYMENT).
        // CPU/wall-clock is bounded by the timeout below on every invocation.

        let source_file = unique_temp_file("pipeline_source", "c");
        let output_file = unique_temp_file("pipeline_output", "o");

        // Write source code
        std::fs::write(&source_file, &request.source_code)?;

        // Compile. Output is read with caps so clang cannot make us buffer an
        // unbounded amount of text (R5 follow-up).
        let mut cmd = Command::new(compiler_path);
        cmd.arg("-c")
            .arg(&source_file)
            .arg("-o")
            .arg(&output_file)
            .arg("-O2")
            .arg("--target=x86_64-unknown-linux-gnu")
            .arg("-fno-asynchronous-unwind-tables");

        let output = timeout(
            Duration::from_secs(request.timeout_secs),
            run_capped(&mut cmd, CHILD_TEXT_CAP, CHILD_STDERR_CAP),
        )
        .await
        .context("Compilation timed out")??;

        if !output.status.success() {
            let _ = std::fs::remove_file(&source_file);
            let _ = std::fs::remove_file(&output_file);
            return Err(anyhow::anyhow!(
                "Compilation failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        // Extract assembly (timeout-bounded), then read the produced object.
        let assembly = self
            .extract_assembly(&output_file, request.timeout_secs)
            .await?;
        let binary = std::fs::read(&output_file)?;

        // Cleanup
        let _ = std::fs::remove_file(&source_file);
        let _ = std::fs::remove_file(&output_file);

        Ok(CompilationResult {
            assembly,
            binary,
            source: request.source_code.clone(),
        })
    }

    async fn extract_assembly(&self, binary_path: &Path, timeout_secs: u64) -> Result<String> {
        let mut cmd = Command::new("objdump");
        cmd.arg("-d").arg("-Mintel").arg(binary_path);

        let output = timeout(
            Duration::from_secs(timeout_secs),
            run_capped(&mut cmd, CHILD_TEXT_CAP, CHILD_STDERR_CAP),
        )
        .await
        .context("objdump timed out")??;

        if !output.status.success() {
            return Err(anyhow::anyhow!(
                "objdump failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }
}

impl Default for CompilerOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
pub struct CompilationRequest {
    pub source_code: String,
    pub compiler: String,
    pub flags: Vec<String>,
    pub timeout_secs: u64,
}

#[derive(Debug, Clone)]
pub struct CompilationResult {
    pub assembly: String,
    pub binary: Vec<u8>,
    pub source: String,
}

#[derive(Debug, Clone, Hash, Eq, PartialEq)]
struct CacheKey {
    hash: String,
    compiler: String,
}

struct CompilationCache {
    inner: lru::LruCache<CacheKey, CompilationResult>,
}

impl CompilationCache {
    fn new(capacity: usize) -> Self {
        Self {
            inner: lru::LruCache::new(std::num::NonZero::new(capacity).unwrap()),
        }
    }

    fn get(&mut self, key: &CacheKey) -> Option<CompilationResult> {
        self.inner.get(key).cloned()
    }

    fn insert(&mut self, key: CacheKey, value: CompilationResult) {
        self.inner.put(key, value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn read_capped_reads_everything_under_cap() {
        let data = b"hello world".to_vec();
        let (buf, truncated) = read_capped(&data[..], 1024).await.unwrap();
        assert_eq!(buf, data);
        assert!(!truncated);
    }

    #[tokio::test]
    async fn read_capped_stops_at_cap() {
        let data = vec![b'x'; 10_000];
        let (buf, truncated) = read_capped(&data[..], 100).await.unwrap();
        assert_eq!(buf.len(), 100, "must not buffer more than the cap");
        assert!(truncated, "hitting the cap must be reported");
    }

    #[tokio::test]
    async fn read_capped_exact_cap_is_not_truncated() {
        let data = [b'y'; 64];
        let (buf, truncated) = read_capped(&data[..], 64).await.unwrap();
        assert_eq!(buf.len(), 64);
        assert!(!truncated);
    }

    #[tokio::test]
    async fn read_capped_zero_cap_yields_empty_and_truncated() {
        let data = [b'z'; 32];
        let (buf, truncated) = read_capped(&data[..], 0).await.unwrap();
        assert!(buf.is_empty());
        assert!(truncated);
    }
}
