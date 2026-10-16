use actix_cors::Cors;
use actix_web::{middleware, web, App, HttpResponse, HttpServer, Responder};
use std::sync::Arc;
use tokio::sync::Semaphore;

mod api;
mod compiler;
mod execution;
mod memory;
mod models;

use api::{compile, execute};

use compiler::{CompilerOrchestrator, EducationalCompiler};
use execution::ExecutionRunner;

/// Default JSON body limit for API requests (256 KB).
///
/// The largest legitimate payload is a 100-line C source file, so this is
/// generous while still rejecting oversized bodies before they are buffered
/// into memory.
const JSON_BODY_LIMIT: usize = 256 * 1024;

/// Below this detected memory limit the server uses the low-memory profile
/// (one worker, one concurrent compile) so at most one `clang` runs at a time.
const LOW_MEMORY_THRESHOLD_BYTES: u64 = 1024 * 1024 * 1024; // 1024 MB

fn env_string(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(default)
}

fn env_u64(key: &str) -> Option<u64> {
    std::env::var(key)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
}

fn env_flag(key: &str) -> bool {
    match std::env::var(key) {
        Ok(value) => matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        Err(_) => false,
    }
}

/// Parse a boolean env var, falling back to `default` when it is unset or
/// unrecognized. An empty value is treated as `false` (safe for kill switches).
fn env_flag_default(key: &str, default: bool) -> bool {
    match std::env::var(key) {
        Ok(value) => match value.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => true,
            "0" | "false" | "no" | "off" | "" => false,
            _ => default,
        },
        Err(_) => default,
    }
}

/// Decide the default `(WORKERS, MAX_CONCURRENT_COMPILES)` profile.
///
/// Uses the low-memory profile `(1, 1)` when `LOW_MEMORY` is set or the
/// detected limit is below [`LOW_MEMORY_THRESHOLD_BYTES`]. `None` means the
/// limit is unknown/unlimited, which uses the larger `(2, 2)` profile.
/// Explicit `WORKERS` / `MAX_CONCURRENT_COMPILES` env vars override this.
fn resolve_default_profile(detected_limit: Option<u64>, low_memory_forced: bool) -> (usize, usize) {
    let small_host = detected_limit
        .map(|bytes| bytes < LOW_MEMORY_THRESHOLD_BYTES)
        .unwrap_or(false);

    if low_memory_forced || small_host {
        (1, 1)
    } else {
        (2, 2)
    }
}

/// Choose the listen address from explicit env values.
///
/// Priority: `BIND_ADDRESS` (explicit) > `PORT` (platform-injected, e.g. Render
/// / Heroku / Koyeb) > `0.0.0.0:8080`. Returns the address and the source name
/// for logging. Pure function so it can be unit-tested without touching env.
fn choose_bind_address(
    bind_address: Option<String>,
    port: Option<String>,
) -> (String, &'static str) {
    if let Some(addr) = bind_address {
        return (addr, "BIND_ADDRESS");
    }
    if let Some(port) = port {
        return (format!("0.0.0.0:{}", port), "PORT");
    }
    ("0.0.0.0:8080".to_string(), "default")
}

/// Resolve the listen address from the environment.
fn resolve_bind_address() -> (String, &'static str) {
    choose_bind_address(
        std::env::var("BIND_ADDRESS").ok(),
        std::env::var("PORT").ok(),
    )
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    // Read environment variables
    let (bind_address, bind_source) = resolve_bind_address();
    let log_level = env_string("LOG_LEVEL", "info");

    env_logger::init_from_env(env_logger::Env::new().default_filter_or(&log_level));

    // Adaptive memory profile: detect the container limit (cgroup v2 -> v1 ->
    // /proc/meminfo) and shrink the worker/compile counts on small hosts so at
    // most one clang process runs at a time. Explicit env vars still win.
    let detected_limit = memory::detect_memory_limit();
    let low_memory_forced = env_flag("LOW_MEMORY");
    let small_host = detected_limit
        .map(|bytes| bytes < LOW_MEMORY_THRESHOLD_BYTES)
        .unwrap_or(false);

    let (default_workers, default_concurrent) =
        resolve_default_profile(detected_limit, low_memory_forced);

    // Hosting-hardening knobs (R2, R3, R5, R7)
    let workers = env_usize("WORKERS", default_workers).max(1);
    let max_concurrent_compiles = env_usize("MAX_CONCURRENT_COMPILES", default_concurrent).max(1);
    let frontend_dir = env_string("FRONTEND_DIR", "./frontend");
    let rate_limit_per_minute = env_usize("RATE_LIMIT_PER_MINUTE", 30);
    let max_ast_bytes = env_usize("MAX_AST_BYTES", 2 * 1024 * 1024);
    // Kill switch for arbitrary code execution (default: enabled).
    let enable_execution = env_flag_default("ENABLE_EXECUTION", true);
    // Resource limits applied to executed programs (never to clang).
    let (max_program_memory_mb, max_program_processes) =
        api::run::resolve_limits(env_u64("MAX_PROGRAM_MEMORY_MB"), env_u64("MAX_PROGRAM_PROCESSES"));
    // Abuse controls.
    let trust_proxy = env_flag_default("TRUST_PROXY", true);
    let max_source_bytes = env_usize("MAX_SOURCE_BYTES", 16 * 1024);
    let run_cooldown_secs = env_u64("RUN_COOLDOWN_SECS").unwrap_or(180);

    let limit_display = match detected_limit {
        Some(bytes) => format!("{} MB", bytes / (1024 * 1024)),
        None => "unlimited".to_string(),
    };

    log::info!("Starting Pipeline backend server...");
    log::info!("Bind address: {} (source: {})", bind_address, bind_source);
    log::info!("Log level: {}", log_level);
    log::info!(
        "Detected memory limit: {} -> WORKERS={}, MAX_CONCURRENT_COMPILES={}",
        limit_display,
        workers,
        max_concurrent_compiles
    );
    if low_memory_forced {
        log::info!("LOW_MEMORY=1 set; forcing the smallest profile (1 worker, 1 concurrent compile).");
    } else if small_host {
        log::info!(
            "Memory limit below {} MB; using the low-memory profile (1 worker, 1 concurrent compile).",
            LOW_MEMORY_THRESHOLD_BYTES / (1024 * 1024)
        );
    }
    log::info!("Rate limit: {}/minute (0 = disabled)", rate_limit_per_minute);
    log::info!("Max AST bytes: {}", max_ast_bytes);
    log::info!(
        "Program execution (/api/run): {}",
        if enable_execution {
            "ENABLED (arbitrary code execution; run the container with --network none)"
        } else {
            "DISABLED (ENABLE_EXECUTION=false)"
        }
    );
    if enable_execution {
        log::info!(
            "Program limits: max memory {} MB, max processes {}, max file 4 MiB \
             (MAX_PROGRAM_MEMORY_MB / MAX_PROGRAM_PROCESSES)",
            max_program_memory_mb,
            max_program_processes
        );
    }
    log::info!(
        "Abuse controls: TRUST_PROXY={}, MAX_SOURCE_BYTES={}, RUN_COOLDOWN_SECS={}{}",
        trust_proxy,
        max_source_bytes,
        run_cooldown_secs,
        if trust_proxy {
            " (client IP from X-Forwarded-For / X-Real-IP)"
        } else {
            " (client IP from the peer address)"
        }
    );
    if run_cooldown_secs == 0 {
        log::info!("Run cooldown disabled (RUN_COOLDOWN_SECS=0)");
    }

    // Shared singletons. These are created exactly once and reused by every
    // worker (R2). Previously they were built inside `HttpServer::new`, so each
    // worker held its own ~1000-entry compilation cache and compiler handles.
    let orchestrator = Arc::new(CompilerOrchestrator::new());
    let edu_compiler = Arc::new(EducationalCompiler::new(max_ast_bytes));
    let runner = Arc::new(ExecutionRunner::new());

    // Global concurrency gate shared across workers (R3). Compile/execute wait
    // for a permit rather than failing.
    let compile_gate = Arc::new(Semaphore::new(max_concurrent_compiles));

    // In-memory per-IP limiter for the API surface (R7).
    let rate_limiter = Arc::new(api::rate_limit::RateLimiter::new(rate_limit_per_minute));

    // Shared abuse-control config + per-IP run cooldown.
    let api_config = Arc::new(api::config::ApiConfig {
        trust_proxy,
        max_source_bytes,
    });
    let cooldown = Arc::new(api::cooldown::Cooldown::new(run_cooldown_secs));

    // Static SPA assets (R1). Falls back to API-only when the directory is absent.
    let frontend = Arc::new(api::static_files::FrontendConfig::new(&frontend_dir));

    // Kill switch state for the /api/run endpoint.
    let run_config = Arc::new(api::run::RunConfig {
        enabled: enable_execution,
        max_memory_mb: max_program_memory_mb,
        max_processes: max_program_processes,
    });

    // R11: never abort when clang is missing. Boot anyway so health checks and
    // the UI still work; /api/compile returns 503 until a compiler is present.
    if !orchestrator.is_available() {
        log::warn!(
            "No C compiler (clang) found. The server will still start and \
             /api/health will respond, but /api/compile will return 503 until a \
             compiler is installed or bundled."
        );
    }

    HttpServer::new(move || {
        let cors = Cors::permissive()
            .allow_any_origin()
            .allow_any_method()
            .allow_any_header()
            // Expose response headers so a cross-origin frontend (split deploy)
            // can read them, e.g. `Retry-After` on a 429 run-cooldown response.
            // `permissive()` already exposes all headers; this is explicit.
            .expose_any_header();

        App::new()
            // `.wrap()` runs in reverse registration order: the last registered
            // is outermost. CORS is registered last so it also decorates
            // short-circuit responses such as 429 rate-limit replies.
            .wrap(api::rate_limit::RateLimit::new(rate_limiter.clone(), trust_proxy))
            .wrap(middleware::Logger::default())
            .wrap(cors)
            .app_data(web::JsonConfig::default().limit(JSON_BODY_LIMIT))
            .app_data(web::Data::new(orchestrator.clone()))
            .app_data(web::Data::new(edu_compiler.clone()))
            .app_data(web::Data::new(runner.clone()))
            .app_data(web::Data::new(compile_gate.clone()))
            .app_data(web::Data::new(frontend.clone()))
            .app_data(web::Data::new(run_config.clone()))
            .app_data(web::Data::new(api_config.clone()))
            .app_data(web::Data::new(cooldown.clone()))
            .service(
                web::scope("/api")
                    .route("/compile", web::post().to(compile))
                    .route("/execute", web::post().to(execute))
                    .route("/run", web::post().to(api::run::run))
                    .route("/health", web::get().to(health_check))
                    // Unknown /api/* paths get a JSON 404 instead of the SPA.
                    .default_service(web::to(api_not_found)),
            )
            .route("/ws", web::get().to(ws_index))
            // Everything not matched above is served from the built frontend
            // (SPA fallback), or a JSON 404 if no frontend is configured.
            .default_service(web::to(api::static_files::serve_static))
    })
    .workers(workers)
    .bind(bind_address)?
    .run()
    .await
}

async fn ws_index(
    req: actix_web::HttpRequest,
    body: web::Payload,
) -> Result<HttpResponse, actix_web::Error> {
    let (response, session, msg_stream) = actix_ws::handle(&req, body)?;

    // Handle the WebSocket session
    tokio::task::spawn_local(async move {
        let _ = api::ws::handle_ws(session, msg_stream).await;
    });

    Ok(response)
}

async fn health_check() -> impl Responder {
    HttpResponse::Ok().json(serde_json::json!({
        "status": "healthy",
        "timestamp": chrono::Utc::now().to_rfc3339(),
        "version": env!("CARGO_PKG_VERSION")
    }))
}

async fn api_not_found() -> impl Responder {
    HttpResponse::NotFound().json(serde_json::json!({
        "error": "not found",
        "message": "unknown API endpoint"
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    // Aliased so the built-in `#[test]` attribute is not shadowed by
    // `actix_web::test` (which is both a module and an attribute macro).
    use actix_web::test as awtest;

    #[actix_web::test]
    async fn health_endpoint_reports_healthy() {
        let app = awtest::init_service(
            App::new().route("/api/health", web::get().to(health_check)),
        )
        .await;

        let req = awtest::TestRequest::get().uri("/api/health").to_request();
        let resp = awtest::call_service(&app, req).await;

        assert!(resp.status().is_success(), "health should be 200 OK");

        let body = awtest::read_body(resp).await;
        let json: serde_json::Value =
            serde_json::from_slice(&body).expect("health body should be JSON");
        assert_eq!(json["status"], "healthy");
    }

    #[test]
    fn low_memory_when_forced() {
        assert_eq!(resolve_default_profile(None, true), (1, 1));
        assert_eq!(resolve_default_profile(Some(u64::MAX), true), (1, 1));
    }

    #[test]
    fn low_memory_when_detected_below_threshold() {
        assert_eq!(resolve_default_profile(Some(512 * 1024 * 1024), false), (1, 1));
        assert_eq!(resolve_default_profile(Some(1023 * 1024 * 1024), false), (1, 1));
    }

    #[test]
    fn larger_profile_when_detected_at_or_above_threshold() {
        assert_eq!(resolve_default_profile(Some(1024 * 1024 * 1024), false), (2, 2));
        assert_eq!(resolve_default_profile(Some(4 * 1024 * 1024 * 1024), false), (2, 2));
    }

    #[test]
    fn larger_profile_when_limit_unknown() {
        assert_eq!(resolve_default_profile(None, false), (2, 2));
    }

    #[test]
    fn bind_address_wins_over_port() {
        let (addr, source) =
            choose_bind_address(Some("0.0.0.0:8080".into()), Some("10000".into()));
        assert_eq!(addr, "0.0.0.0:8080");
        assert_eq!(source, "BIND_ADDRESS");
    }

    #[test]
    fn port_used_when_bind_address_absent() {
        let (addr, source) = choose_bind_address(None, Some("10000".into()));
        assert_eq!(addr, "0.0.0.0:10000");
        assert_eq!(source, "PORT");
    }

    #[test]
    fn default_used_when_neither_set() {
        let (addr, source) = choose_bind_address(None, None);
        assert_eq!(addr, "0.0.0.0:8080");
        assert_eq!(source, "default");
    }
}
