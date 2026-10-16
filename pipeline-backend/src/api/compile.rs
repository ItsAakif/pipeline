use actix_web::{web, HttpResponse, Responder};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Semaphore;

use crate::compiler::{CompilerOrchestrator, CompilationRequest as CompilerRequest, EducationalCompiler};
use crate::models::{ExecutionFrame, CompilationStages};
use base64::prelude::*;

#[derive(Deserialize)]
pub struct CompileRequest {
    pub source_code: String,
    #[serde(default = "default_compiler")]
    pub compiler: String,
    #[serde(default = "Vec::new")]
    pub flags: Vec<String>,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

fn default_compiler() -> String {
    "clang".to_string()
}

fn default_timeout() -> u64 {
    10
}

#[derive(Serialize)]
pub struct CompileResponse {
    pub success: bool,
    pub assembly: Option<String>,
    pub source: Option<String>,
    pub binary: Option<String>, // Base64 encoded
    pub stages: Option<CompilationStages>, // NEW: All compilation stages
    pub error: Option<String>,
    pub compilation_time_ms: u128,
}

fn compile_failure(
    status: actix_web::http::StatusCode,
    message: String,
    start: std::time::Instant,
) -> HttpResponse {
    HttpResponse::build(status).json(CompileResponse {
        success: false,
        assembly: None,
        source: None,
        binary: None,
        stages: None,
        error: Some(message),
        compilation_time_ms: start.elapsed().as_millis(),
    })
}

pub async fn compile(
    req: web::Json<CompileRequest>,
    limits: web::Data<Arc<crate::api::config::ApiConfig>>,
    orchestrator: web::Data<Arc<CompilerOrchestrator>>,
    edu_compiler: web::Data<Arc<EducationalCompiler>>,
    compile_gate: web::Data<Arc<Semaphore>>,
) -> impl Responder {
    let start = std::time::Instant::now();

    log::info!("Compilation request received");

    // Validate source code is not empty
    if req.source_code.trim().is_empty() {
        return compile_failure(
            actix_web::http::StatusCode::BAD_REQUEST,
            "Source code cannot be empty".to_string(),
            start,
        );
    }

    // Validate byte size (MAX_SOURCE_BYTES)
    if req.source_code.len() > limits.max_source_bytes {
        return compile_failure(
            actix_web::http::StatusCode::BAD_REQUEST,
            format!(
                "Code exceeds the {} byte limit",
                limits.max_source_bytes
            ),
            start,
        );
    }

    // Validate code length (100 line limit)
    if req.source_code.lines().count() > 100 {
        return compile_failure(
            actix_web::http::StatusCode::BAD_REQUEST,
            "Code exceeds 100 line limit".to_string(),
            start,
        );
    }

    // Validate compiler name
    let allowed_compilers = ["clang", "gcc"];
    if !allowed_compilers.contains(&req.compiler.as_str()) {
        return compile_failure(
            actix_web::http::StatusCode::BAD_REQUEST,
            format!(
                "Unsupported compiler: {}. Allowed: {}",
                req.compiler,
                allowed_compilers.join(", ")
            ),
            start,
        );
    }

    // Validate timeout is reasonable (between 1 and 60 seconds)
    if req.timeout_secs < 1 || req.timeout_secs > 60 {
        return compile_failure(
            actix_web::http::StatusCode::BAD_REQUEST,
            "Timeout must be between 1 and 60 seconds".to_string(),
            start,
        );
    }

    // R11: no compiler available -> clear 503 rather than a 500/crash.
    if !orchestrator.is_available() {
        log::warn!("Rejecting compile request: no compiler available");
        return compile_failure(
            actix_web::http::StatusCode::SERVICE_UNAVAILABLE,
            "No C compiler is available on this server. \
             Please try again later or contact the administrator."
                .to_string(),
            start,
        );
    }

    // R3: bound concurrency globally. Wait for a permit instead of failing.
    let _permit = match compile_gate.acquire().await {
        Ok(permit) => permit,
        Err(_) => {
            return compile_failure(
                actix_web::http::StatusCode::SERVICE_UNAVAILABLE,
                "Compilation service is shutting down".to_string(),
                start,
            );
        }
    };

    let compiler_request = CompilerRequest {
        source_code: req.source_code.clone(),
        compiler: req.compiler.clone(),
        flags: req.flags.clone(),
        timeout_secs: req.timeout_secs,
    };

    match orchestrator.compile(compiler_request).await {
        Ok(result) => {
            let elapsed = start.elapsed().as_millis();
            log::info!("Compilation successful in {}ms", elapsed);

            // Extract educational stages, reusing the object file the
            // orchestrator just produced instead of compiling a third time (R4).
            let stages = match edu_compiler
                .compile_with_stages(&req.source_code, req.timeout_secs, Some(&result.binary))
                .await
            {
                Ok(s) => {
                    log::info!("Educational stages extracted successfully");
                    Some(s)
                }
                Err(e) => {
                    log::warn!("Failed to extract educational stages: {}", e);
                    None
                }
            };

            HttpResponse::Ok().json(CompileResponse {
                success: true,
                assembly: Some(result.assembly),
                source: Some(result.source),
                binary: Some(BASE64_STANDARD.encode(&result.binary)),
                stages,
                error: None,
                compilation_time_ms: elapsed,
            })
        }
        Err(e) => {
            log::error!("Compilation failed: {}", e);

            HttpResponse::InternalServerError().json(CompileResponse {
                success: false,
                assembly: None,
                source: None,
                binary: None,
                stages: None,
                error: Some(format!(
                    "Compilation error: {}",
                    e.chain()
                        .map(|c| c.to_string())
                        .collect::<Vec<_>>()
                        .join(" -> ")
                )),
                compilation_time_ms: start.elapsed().as_millis(),
            })
        }
    }
}

#[derive(Deserialize)]
pub struct ExecuteRequest {
    pub binary: String, // Base64 encoded
}

#[derive(Serialize)]
pub struct ExecuteResponse {
    pub success: bool,
    pub frames: Vec<ExecutionFrame>,
    pub frame_count: usize,
    pub error: Option<String>,
    pub execution_time_ms: u128,
}

pub async fn execute(
    req: web::Json<ExecuteRequest>,
    runner: web::Data<Arc<crate::execution::ExecutionRunner>>,
    compile_gate: web::Data<Arc<Semaphore>>,
) -> impl Responder {
    let start = std::time::Instant::now();

    log::info!("Execution request received");

    // R3: `/api/execute` shares the same gate as `/api/compile`. Execution is
    // pure in-process CPU/decoding work, so a shared (small) limit keeps total
    // concurrent work bounded on a low-memory host. Waiting rather than failing.
    let _permit = match compile_gate.acquire().await {
        Ok(permit) => permit,
        Err(_) => {
            return HttpResponse::ServiceUnavailable().json(ExecuteResponse {
                success: false,
                frames: vec![],
                frame_count: 0,
                error: Some("Execution service is shutting down".to_string()),
                execution_time_ms: start.elapsed().as_millis(),
            });
        }
    };

    match BASE64_STANDARD.decode(&req.binary) {
        Ok(binary) => {
            match runner.run(&binary).await {
                Ok(frames) => {
                    let elapsed = start.elapsed().as_millis();
                    let frame_count = frames.len();

                    log::info!("Execution completed with {} frames in {}ms", frame_count, elapsed);

                    HttpResponse::Ok().json(ExecuteResponse {
                        success: true,
                        frames,
                        frame_count,
                        error: None,
                        execution_time_ms: elapsed,
                    })
                }
                Err(e) => {
                    log::error!("Execution failed: {}", e);

                    HttpResponse::InternalServerError().json(ExecuteResponse {
                        success: false,
                        frames: vec![],
                        frame_count: 0,
                        error: Some(e.to_string()),
                        execution_time_ms: start.elapsed().as_millis(),
                    })
                }
            }
        }
        Err(e) => {
            HttpResponse::BadRequest().json(ExecuteResponse {
                success: false,
                frames: vec![],
                frame_count: 0,
                error: Some(format!("Invalid base64: {}", e)),
                execution_time_ms: start.elapsed().as_millis(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::config::ApiConfig;
    use actix_web::{test as awtest, web, App};

    #[actix_web::test]
    async fn oversized_source_is_rejected() {
        let api_config = Arc::new(ApiConfig {
            trust_proxy: false,
            max_source_bytes: 16 * 1024,
        });
        let orchestrator = Arc::new(CompilerOrchestrator::new());
        let edu = Arc::new(EducationalCompiler::new(2 * 1024 * 1024));
        let gate = Arc::new(Semaphore::new(1));

        let app = awtest::init_service(
            App::new()
                .app_data(web::Data::new(api_config))
                .app_data(web::Data::new(orchestrator))
                .app_data(web::Data::new(edu))
                .app_data(web::Data::new(gate))
                .route("/api/compile", web::post().to(compile)),
        )
        .await;

        // One line, but larger than the 16 KiB limit.
        let source = format!("int x;{}", " ".repeat(20 * 1024));
        let req = awtest::TestRequest::post()
            .uri("/api/compile")
            .set_json(serde_json::json!({ "source_code": source }))
            .to_request();
        let resp = awtest::call_service(&app, req).await;

        assert_eq!(resp.status().as_u16(), 400, "oversized body must be 400");
        let body = awtest::read_body(resp).await;
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["success"], false);
        let message = json["error"].as_str().unwrap();
        assert!(
            message.contains("16384") && message.contains("byte limit"),
            "message should name the limit: {message}"
        );
    }
}
