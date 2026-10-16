//! `POST /api/run` — compile, link, and actually execute the user's program.
//!
//! SECURITY: this endpoint performs arbitrary code execution. Mitigations:
//! * wall-clock timeout (tokio) — the child is killed when it expires;
//! * CPU-time limit (`ulimit -t`) via a `/bin/bash -c` wrapper;
//! * address-space limit (`ulimit -v`, `MAX_PROGRAM_MEMORY_MB`);
//! * process-count limit (`ulimit -u`, `MAX_PROGRAM_PROCESSES`) — contains
//!   fork bombs;
//! * file-size limit (`ulimit -f 8192`, i.e. 4 MiB);
//! * 64 KiB output caps per stream — the child is killed as soon as either cap
//!   is reached;
//! * the child runs in its own process group, and on timeout/cap the whole
//!   group is killed so forked grandchildren do not survive;
//! * the shared global compile semaphore and the per-IP rate limiter;
//! * the `ENABLE_EXECUTION` kill switch (403 when disabled).
//!
//! The container should additionally be run with `--network none`,
//! `--pids-limit`, and `--memory` (see SECURITY.md).

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use actix_web::{web, HttpRequest, HttpResponse, Responder};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::sync::Semaphore;
use tokio::task::JoinHandle;
use tokio::time::timeout;

use crate::api::config::ApiConfig;
use crate::api::cooldown::Cooldown;
use crate::api::rate_limit::client_ip;
use crate::compiler::orchestrator::{run_capped, unique_temp_dir};
use crate::compiler::CompilerOrchestrator;

/// Default wall-clock timeout when the request omits `timeout_secs`.
const DEFAULT_TIMEOUT_SECS: u64 = 5;
/// Allowed timeout range (values are clamped into it).
const MIN_TIMEOUT_SECS: u64 = 1;
const MAX_TIMEOUT_SECS: u64 = 10;
/// Hard ceiling for the compile+link step (independent of the run timeout).
const COMPILE_TIMEOUT_SECS: u64 = 30;
/// Output cap per stream (stdout and stderr) for both compile and run.
const OUTPUT_CAP_BYTES: usize = 64 * 1024;
const SRC_FILE_NAME: &str = "source.c";
const PROG_FILE_NAME: &str = "prog";
/// `ulimit -f` value, in 512-byte blocks (8192 blocks = 4 MiB).
const MAX_FILE_BLOCKS: u64 = 8192;

/// Default address-space limit for the executed program (256 MB).
pub const DEFAULT_MAX_MEMORY_MB: u64 = 256;
pub const MIN_MAX_MEMORY_MB: u64 = 16;
pub const MAX_MAX_MEMORY_MB: u64 = 4096;
/// Default process-count limit for the executed program.
///
/// Kept well below a typical container `--pids-limit` (e.g. 256) so a fork bomb
/// is stopped before it can exhaust the container's PID space.
pub const DEFAULT_MAX_PROCESSES: u64 = 128;
pub const MIN_MAX_PROCESSES: u64 = 1;
pub const MAX_MAX_PROCESSES: u64 = 4096;

fn default_timeout() -> u64 {
    DEFAULT_TIMEOUT_SECS
}

/// Clamp the requested timeout into the supported range.
fn clamp_timeout(timeout_secs: u64) -> u64 {
    timeout_secs.clamp(MIN_TIMEOUT_SECS, MAX_TIMEOUT_SECS)
}

pub fn clamp_memory_mb(memory_mb: u64) -> u64 {
    memory_mb.clamp(MIN_MAX_MEMORY_MB, MAX_MAX_MEMORY_MB)
}

pub fn clamp_processes(processes: u64) -> u64 {
    processes.clamp(MIN_MAX_PROCESSES, MAX_MAX_PROCESSES)
}

/// Resolve the program resource limits from optional env values, applying the
/// defaults and clamping into the safe ranges. Pure (no env access) so it can
/// be unit-tested.
pub fn resolve_limits(memory_mb: Option<u64>, processes: Option<u64>) -> (u64, u64) {
    (
        clamp_memory_mb(memory_mb.unwrap_or(DEFAULT_MAX_MEMORY_MB)),
        clamp_processes(processes.unwrap_or(DEFAULT_MAX_PROCESSES)),
    )
}

/// The `/bin/bash -c` wrapper that applies the per-run resource limits and then
/// replaces the shell with the program (`exec`).
fn program_wrapper_command(timeout_secs: u64, max_memory_mb: u64, max_processes: u64) -> String {
    format!(
        "ulimit -t {timeout_secs}; ulimit -v {mem_kb}; ulimit -u {max_processes}; \
         ulimit -f {MAX_FILE_BLOCKS}; exec ./{PROG_FILE_NAME}",
        mem_kb = max_memory_mb.saturating_mul(1024),
    )
}

#[derive(Deserialize)]
pub struct RunRequest {
    pub source_code: String,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

#[derive(Serialize)]
pub struct RunResponse {
    pub success: bool,
    pub compile_error: Option<String>,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub timed_out: bool,
    pub truncated: bool,
    pub execution_time_ms: u128,
}

/// Startup configuration for the run endpoint.
pub struct RunConfig {
    /// `ENABLE_EXECUTION` (default true).
    pub enabled: bool,
    /// `MAX_PROGRAM_MEMORY_MB` (default 256).
    pub max_memory_mb: u64,
    /// `MAX_PROGRAM_PROCESSES` (default 128).
    pub max_processes: u64,
}

/// Removes the per-request temp dir on drop. A `Drop` guard is cancellation
/// safe: if the handler future is dropped (e.g. the client disconnects) the dir
/// is still removed.
struct TempDirGuard {
    path: PathBuf,
}

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn error_response(status: actix_web::http::StatusCode, message: impl Into<String>) -> HttpResponse {
    HttpResponse::build(status).json(RunResponse {
        success: false,
        compile_error: Some(message.into()),
        stdout: String::new(),
        stderr: String::new(),
        exit_code: -1,
        timed_out: false,
        truncated: false,
        execution_time_ms: 0,
    })
}

/// 429 response for an active per-IP run cooldown.
fn cooldown_response(retry_after_secs: u64) -> HttpResponse {
    HttpResponse::TooManyRequests()
        .insert_header((
            actix_web::http::header::RETRY_AFTER,
            retry_after_secs.to_string(),
        ))
        .json(serde_json::json!({
            "error": "run cooldown active",
            "retry_after_secs": retry_after_secs,
        }))
}

pub async fn run(
    req: web::Json<RunRequest>,
    http: HttpRequest,
    config: web::Data<Arc<RunConfig>>,
    limits: web::Data<Arc<ApiConfig>>,
    cooldown: web::Data<Arc<Cooldown>>,
    orchestrator: web::Data<Arc<CompilerOrchestrator>>,
    compile_gate: web::Data<Arc<Semaphore>>,
) -> impl Responder {
    let start = std::time::Instant::now();

    // Kill switch: run nothing when execution is disabled.
    if !config.enabled {
        log::warn!("Rejecting /api/run: program execution is disabled (ENABLE_EXECUTION=false)");
        return error_response(
            actix_web::http::StatusCode::FORBIDDEN,
            "Program execution is disabled on this server (ENABLE_EXECUTION=false).",
        );
    }

    // Validation mirrors /api/compile.
    if req.source_code.trim().is_empty() {
        return error_response(
            actix_web::http::StatusCode::BAD_REQUEST,
            "Source code cannot be empty",
        );
    }
    if req.source_code.len() > limits.max_source_bytes {
        return error_response(
            actix_web::http::StatusCode::BAD_REQUEST,
            format!("Code exceeds the {} byte limit", limits.max_source_bytes),
        );
    }
    if req.source_code.lines().count() > 100 {
        return error_response(
            actix_web::http::StatusCode::BAD_REQUEST,
            "Code exceeds 100 line limit",
        );
    }

    let timeout_secs = clamp_timeout(req.timeout_secs);

    // Reuse the compiler discovered at startup (same resolution as /api/compile).
    let Some(compiler) = orchestrator.compiler_path() else {
        log::warn!("Rejecting /api/run: no compiler available");
        return error_response(
            actix_web::http::StatusCode::SERVICE_UNAVAILABLE,
            "No C compiler is available on this server. \
             Please try again later or contact the administrator.",
        );
    };
    let compiler = compiler.to_path_buf();

    // Per-IP cooldown: after a run is accepted, further runs are rejected for
    // RUN_COOLDOWN_SECS. Checked before the semaphore so it fails fast.
    let ip = client_ip(http.headers(), http.peer_addr(), limits.trust_proxy);
    if let Err(retry_after_secs) = cooldown.acquire(ip) {
        log::warn!(
            "Rejecting /api/run for {}: cooldown active ({}s remaining)",
            ip,
            retry_after_secs
        );
        return cooldown_response(retry_after_secs);
    }

    // Bound concurrency with the shared global gate (wait rather than fail).
    let _permit = match compile_gate.acquire().await {
        Ok(permit) => permit,
        Err(_) => {
            return error_response(
                actix_web::http::StatusCode::SERVICE_UNAVAILABLE,
                "Execution service is shutting down",
            );
        }
    };

    log::info!(
        "Run request received (timeout {}s, mem {} MB, procs {})",
        timeout_secs,
        config.max_memory_mb,
        config.max_processes
    );

    // Per-request temp dir; the guard removes it on every path, including
    // cancellation.
    let work_dir = unique_temp_dir("pipeline_run_work");
    if let Err(e) = std::fs::create_dir_all(&work_dir) {
        log::error!("Failed to create temp dir {}: {}", work_dir.display(), e);
        return error_response(
            actix_web::http::StatusCode::INTERNAL_SERVER_ERROR,
            "Failed to prepare the execution sandbox",
        );
    }
    let _guard = TempDirGuard {
        path: work_dir.clone(),
    };

    let response = execute_in_dir(
        &compiler,
        &work_dir,
        &req.source_code,
        timeout_secs,
        config.max_memory_mb,
        config.max_processes,
    )
    .await;

    let elapsed = start.elapsed().as_millis();
    log::info!(
        "Run finished: success={} exit_code={} timed_out={} truncated={} in {}ms",
        response.success,
        response.exit_code,
        response.timed_out,
        response.truncated,
        elapsed
    );

    HttpResponse::Ok().json(RunResponse {
        execution_time_ms: elapsed,
        ..response
    })
}

/// Compile+link `source.c` into `prog` and run it. The caller owns the temp
/// dir's lifetime (see [`TempDirGuard`]).
async fn execute_in_dir(
    compiler: &Path,
    work_dir: &Path,
    source_code: &str,
    timeout_secs: u64,
    max_memory_mb: u64,
    max_processes: u64,
) -> RunResponse {
    let source_file = work_dir.join(SRC_FILE_NAME);
    if let Err(e) = std::fs::write(&source_file, source_code) {
        log::error!("Failed to write source file: {}", e);
        return failure(format!("Failed to write source file: {}", e));
    }

    // 1. Compile AND link into a native executable. No `--target`: the binary
    //    must run in this container. The resource limits below apply only to the
    //    executed program, never to clang.
    let mut compile_cmd = Command::new(compiler);
    compile_cmd
        .current_dir(work_dir)
        .arg("-O0")
        .arg("-o")
        .arg(PROG_FILE_NAME)
        .arg(SRC_FILE_NAME);

    let compile = timeout(
        Duration::from_secs(COMPILE_TIMEOUT_SECS),
        run_capped(&mut compile_cmd, OUTPUT_CAP_BYTES, OUTPUT_CAP_BYTES),
    )
    .await;

    match compile {
        Ok(Ok(output)) if output.status.success() => {}
        Ok(Ok(output)) => {
            let diagnostics = String::from_utf8_lossy(&output.stderr).to_string();
            return failure(if diagnostics.trim().is_empty() {
                "Compilation failed".to_string()
            } else {
                diagnostics
            });
        }
        Ok(Err(e)) => return failure(format!("Failed to run compiler: {}", e)),
        Err(_) => return failure("Compilation timed out".to_string()),
    }

    // 2. Run `./prog` under the resource limits, a wall-clock timeout, and
    //    per-stream output caps. `exec` replaces the shell with the program.
    let shell_command = program_wrapper_command(timeout_secs, max_memory_mb, max_processes);
    let mut run_cmd = Command::new("/bin/bash");
    run_cmd
        .current_dir(work_dir)
        .arg("-c")
        .arg(&shell_command);

    match run_program(
        &mut run_cmd,
        OUTPUT_CAP_BYTES,
        OUTPUT_CAP_BYTES,
        Duration::from_secs(timeout_secs),
    )
    .await
    {
        Ok(outcome) => RunResponse {
            success: true,
            compile_error: None,
            stdout: String::from_utf8_lossy(&outcome.stdout).to_string(),
            stderr: String::from_utf8_lossy(&outcome.stderr).to_string(),
            // Killed by a signal (e.g. `ulimit -t`) has no exit code.
            exit_code: outcome.status.and_then(|status| status.code()).unwrap_or(-1),
            timed_out: outcome.timed_out,
            // A timed-out run is reported as truncated too: the captured output
            // is incomplete.
            truncated: outcome.truncated || outcome.timed_out,
            execution_time_ms: 0,
        },
        Err(e) => failure(format!("Failed to run program: {}", e)),
    }
}

fn failure(message: String) -> RunResponse {
    RunResponse {
        success: false,
        compile_error: Some(message),
        stdout: String::new(),
        stderr: String::new(),
        exit_code: -1,
        timed_out: false,
        truncated: false,
        execution_time_ms: 0,
    }
}

/// Result of running a program with caps, a timeout, and process-group kills.
struct ProgramOutcome {
    status: Option<std::process::ExitStatus>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    truncated: bool,
    timed_out: bool,
}

/// Put the child in its own process group (Unix).
#[cfg(unix)]
fn set_process_group(cmd: &mut Command) {
    cmd.process_group(0);
}

#[cfg(not(unix))]
fn set_process_group(_cmd: &mut Command) {}

/// Kill an entire process group (negative pid). Unix only.
#[cfg(unix)]
fn kill_process_group(pid: Option<u32>) {
    if let Some(pid) = pid {
        use nix::sys::signal::{kill, Signal};
        use nix::unistd::Pid;
        let _ = kill(Pid::from_raw(-(pid as i32)), Signal::SIGKILL);
    }
}

#[cfg(not(unix))]
fn kill_process_group(_pid: Option<u32>) {}

/// Spawn a task that drains one pipe into a shared buffer, killing the whole
/// process group as soon as the cap is reached.
fn spawn_reader<R>(
    reader: Option<R>,
    buffer: Arc<Mutex<Vec<u8>>>,
    truncated: Arc<AtomicBool>,
    cap: usize,
    pid: Option<u32>,
) -> JoinHandle<()>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let Some(mut reader) = reader else { return };

        let mut chunk = [0u8; 8192];
        loop {
            match reader.read(&mut chunk).await {
                Ok(0) => break,
                Ok(n) => {
                    let mut buffer = buffer.lock().unwrap();
                    if buffer.len() + n > cap {
                        let remaining = cap.saturating_sub(buffer.len());
                        buffer.extend_from_slice(&chunk[..remaining]);
                        drop(buffer);

                        // Kill immediately: either stream reaching its cap ends
                        // the whole process group.
                        truncated.store(true, Ordering::SeqCst);
                        kill_process_group(pid);
                        break;
                    }
                    buffer.extend_from_slice(&chunk[..n]);
                }
                Err(_) => break,
            }
        }
    })
}

/// Run `cmd` with output caps and a wall-clock timeout.
///
/// The child is placed in its own process group; on timeout or when either
/// output stream reaches its cap the whole group is killed (falling back to
/// `child.kill()`), so forked grandchildren do not survive. Already-captured
/// output is always surfaced, including when the timeout fires.
async fn run_program(
    cmd: &mut Command,
    stdout_cap: usize,
    stderr_cap: usize,
    wall: Duration,
) -> std::io::Result<ProgramOutcome> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    set_process_group(cmd);

    let mut child = cmd.spawn()?;
    let pid = child.id();

    let out_buf = Arc::new(Mutex::new(Vec::<u8>::new()));
    let err_buf = Arc::new(Mutex::new(Vec::<u8>::new()));
    let truncated = Arc::new(AtomicBool::new(false));

    // Readers own the pipes and run independently of the timeout, so their
    // buffers survive even when the wall-clock timeout fires.
    let out_task = spawn_reader(
        child.stdout.take(),
        out_buf.clone(),
        truncated.clone(),
        stdout_cap,
        pid,
    );
    let err_task = spawn_reader(
        child.stderr.take(),
        err_buf.clone(),
        truncated.clone(),
        stderr_cap,
        pid,
    );

    let mut status = None;
    let mut timed_out = false;

    match timeout(wall, child.wait()).await {
        Ok(Ok(exit_status)) => status = Some(exit_status),
        Ok(Err(_)) => {}
        Err(_) => {
            timed_out = true;
            // Kill the group first so forked children die, then reap the child.
            kill_process_group(pid);
            let _ = child.kill().await;
            if let Ok(exit_status) = child.wait().await {
                status = Some(exit_status);
            }
        }
    }

    // Once the child is gone the pipes close and the readers finish; if a cap
    // was hit the group was already killed, so they finish too. Bound the joins
    // so a process that escaped the group (e.g. via `setsid`) and still holds a
    // pipe open cannot hang the request.
    let _ = timeout(Duration::from_secs(2), out_task).await;
    let _ = timeout(Duration::from_secs(2), err_task).await;

    let stdout = out_buf.lock().unwrap().clone();
    let stderr = err_buf.lock().unwrap().clone();

    Ok(ProgramOutcome {
        status,
        stdout,
        stderr,
        truncated: truncated.load(Ordering::SeqCst),
        timed_out,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::{test as awtest, web, App};

    fn init_app(
        enabled: bool,
    ) -> App<
        impl actix_web::dev::ServiceFactory<
            actix_web::dev::ServiceRequest,
            Config = (),
            InitError = (),
            Error = actix_web::Error,
            Response = actix_web::dev::ServiceResponse,
        >,
    > {
        let config = Arc::new(RunConfig {
            enabled,
            max_memory_mb: DEFAULT_MAX_MEMORY_MB,
            max_processes: DEFAULT_MAX_PROCESSES,
        });
        let api_config = Arc::new(ApiConfig {
            trust_proxy: false,
            max_source_bytes: 16 * 1024,
        });
        // Cooldown disabled for the endpoint tests.
        let cooldown = Arc::new(Cooldown::new(0));
        let orchestrator = Arc::new(CompilerOrchestrator::new());
        let gate = Arc::new(Semaphore::new(1));

        App::new()
            .app_data(web::Data::new(config))
            .app_data(web::Data::new(api_config))
            .app_data(web::Data::new(cooldown))
            .app_data(web::Data::new(orchestrator))
            .app_data(web::Data::new(gate))
            .route("/api/run", web::post().to(run))
    }

    #[test]
    fn clamps_timeout_into_range() {
        assert_eq!(clamp_timeout(0), 1);
        assert_eq!(clamp_timeout(1), 1);
        assert_eq!(clamp_timeout(5), 5);
        assert_eq!(clamp_timeout(10), 10);
        assert_eq!(clamp_timeout(11), 10);
        assert_eq!(clamp_timeout(u64::MAX), 10);
    }

    #[test]
    fn resolve_limits_uses_defaults() {
        assert_eq!(
            resolve_limits(None, None),
            (DEFAULT_MAX_MEMORY_MB, DEFAULT_MAX_PROCESSES)
        );
        assert_eq!(resolve_limits(None, None), (256, 128));
    }

    #[test]
    fn resolve_limits_clamps_into_safe_range() {
        assert_eq!(resolve_limits(Some(0), Some(0)), (MIN_MAX_MEMORY_MB, MIN_MAX_PROCESSES));
        assert_eq!(
            resolve_limits(Some(u64::MAX), Some(u64::MAX)),
            (MAX_MAX_MEMORY_MB, MAX_MAX_PROCESSES)
        );
        assert_eq!(resolve_limits(Some(1), Some(1)), (MIN_MAX_MEMORY_MB, MIN_MAX_PROCESSES));
    }

    #[test]
    fn resolve_limits_passes_through_valid_values() {
        assert_eq!(resolve_limits(Some(128), Some(64)), (128, 64));
        assert_eq!(resolve_limits(Some(1024), Some(1024)), (1024, 1024));
    }

    #[test]
    fn clamps_individual_limits() {
        assert_eq!(clamp_memory_mb(0), 16);
        assert_eq!(clamp_memory_mb(256), 256);
        assert_eq!(clamp_memory_mb(99_999), 4096);
        assert_eq!(clamp_processes(0), 1);
        assert_eq!(clamp_processes(512), 512);
        assert_eq!(clamp_processes(99_999), 4096);
    }

    #[test]
    fn wrapper_command_sets_all_limits() {
        let cmd = program_wrapper_command(5, 256, 512);
        assert_eq!(
            cmd,
            "ulimit -t 5; ulimit -v 262144; ulimit -u 512; ulimit -f 8192; exec ./prog"
        );
        assert!(cmd.contains("ulimit -v 262144"), "256 MB in KiB");
        assert!(cmd.contains("ulimit -u 512"));
        assert!(cmd.contains("ulimit -f 8192"));
        assert!(cmd.ends_with("exec ./prog"));
    }

    #[test]
    fn response_shape_is_stable() {
        let json = serde_json::to_value(RunResponse {
            success: true,
            compile_error: None,
            stdout: "hi\n".to_string(),
            stderr: String::new(),
            exit_code: 0,
            timed_out: false,
            truncated: false,
            execution_time_ms: 12,
        })
        .unwrap();

        for key in [
            "success",
            "compile_error",
            "stdout",
            "stderr",
            "exit_code",
            "timed_out",
            "truncated",
            "execution_time_ms",
        ] {
            assert!(json.get(key).is_some(), "missing field {key}");
        }
        assert_eq!(json["success"], true);
        assert_eq!(json["compile_error"], serde_json::Value::Null);
        assert_eq!(json["stdout"], "hi\n");
        assert_eq!(json["exit_code"], 0);
        assert_eq!(json["timed_out"], false);
        assert_eq!(json["truncated"], false);
    }

    #[actix_web::test]
    async fn disabled_returns_403_and_runs_nothing() {
        let app = awtest::init_service(init_app(false)).await;

        let req = awtest::TestRequest::post()
            .uri("/api/run")
            .set_json(serde_json::json!({ "source_code": "int main(){return 0;}" }))
            .to_request();
        let resp = awtest::call_service(&app, req).await;

        assert_eq!(resp.status().as_u16(), 403, "disabled execution must be 403");

        let body = awtest::read_body(resp).await;
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["success"], false);
        let message = json["compile_error"].as_str().unwrap_or_default().to_lowercase();
        assert!(message.contains("disabled"), "message should be clear: {message}");
    }

    #[actix_web::test]
    async fn empty_source_is_rejected() {
        let app = awtest::init_service(init_app(true)).await;

        let req = awtest::TestRequest::post()
            .uri("/api/run")
            .set_json(serde_json::json!({ "source_code": "   " }))
            .to_request();
        let resp = awtest::call_service(&app, req).await;

        assert_eq!(resp.status().as_u16(), 400);
        let body = awtest::read_body(resp).await;
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["success"], false);
        assert!(json["compile_error"].as_str().unwrap().contains("empty"));
    }

    #[actix_web::test]
    async fn over_100_lines_is_rejected() {
        let app = awtest::init_service(init_app(true)).await;

        let source = "int x;\n".repeat(101);
        let req = awtest::TestRequest::post()
            .uri("/api/run")
            .set_json(serde_json::json!({ "source_code": source }))
            .to_request();
        let resp = awtest::call_service(&app, req).await;

        assert_eq!(resp.status().as_u16(), 400);
        let body = awtest::read_body(resp).await;
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["success"], false);
        assert!(json["compile_error"].as_str().unwrap().contains("100 line"));
    }

    #[actix_web::test]
    async fn oversized_source_is_rejected() {
        let app = awtest::init_service(init_app(true)).await;

        // One line, but larger than the 16 KiB MAX_SOURCE_BYTES default.
        let source = format!("int x;{}", " ".repeat(20 * 1024));
        let req = awtest::TestRequest::post()
            .uri("/api/run")
            .set_json(serde_json::json!({ "source_code": source }))
            .to_request();
        let resp = awtest::call_service(&app, req).await;

        assert_eq!(resp.status().as_u16(), 400, "oversized body must be 400");
        let body = awtest::read_body(resp).await;
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["success"], false);
        let message = json["compile_error"].as_str().unwrap();
        assert!(
            message.contains("16384") && message.contains("byte limit"),
            "message should name the limit: {message}"
        );
    }
}
