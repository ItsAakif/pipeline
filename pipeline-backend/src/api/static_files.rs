use std::path::{Path, PathBuf};

use actix_web::{http::Method, web, HttpRequest, HttpResponse, Responder};

/// Resolved static-asset root for the built SPA.
///
/// `root` is `None` when no frontend directory was found at startup, in which
/// case the backend serves API responses only (split-deploy keeps working).
pub struct FrontendConfig {
    pub root: Option<PathBuf>,
}

impl FrontendConfig {
    pub fn new(dir: &str) -> Self {
        let path = Path::new(dir);
        if path.is_dir() {
            match std::fs::canonicalize(path) {
                Ok(root) => {
                    log::info!("Serving frontend assets from {}", root.display());
                    return Self { root: Some(root) };
                }
                Err(e) => {
                    log::warn!("FRONTEND_DIR '{}' could not be resolved: {}", dir, e);
                }
            }
        } else {
            log::warn!(
                "FRONTEND_DIR '{}' not found; serving API only (split deploy)",
                dir
            );
        }

        Self { root: None }
    }
}

/// Map a file path to its `Content-Type`.
fn mime_for(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
        .as_deref()
    {
        Some("html") | Some("htm") => "text/html; charset=utf-8",
        Some("js") | Some("mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") | Some("map") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        Some("woff") => "font/woff",
        Some("ttf") => "font/ttf",
        Some("otf") => "font/otf",
        Some("wasm") => "application/wasm",
        Some("txt") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// Serve a built SPA file, falling back to `index.html` for client-side routes.
///
/// Route precedence is enforced by the caller: `/api/*` and `/ws` are handled
/// first, and this handler is the app-level default service.
pub async fn serve_static(req: HttpRequest, cfg: web::Data<std::sync::Arc<FrontendConfig>>) -> impl Responder {
    if req.method() != Method::GET && req.method() != Method::HEAD {
        return HttpResponse::MethodNotAllowed()
            .json(serde_json::json!({ "error": "method not allowed" }));
    }

    let Some(root) = cfg.root.as_ref() else {
        return HttpResponse::NotFound().json(serde_json::json!({
            "error": "not found",
            "message": "frontend assets are not available"
        }));
    };

    let rel = req.path().trim_start_matches('/');

    // Reject path traversal before touching the filesystem.
    if rel.split('/').any(|segment| segment == "..") || rel.contains('\\') {
        return HttpResponse::NotFound().json(serde_json::json!({ "error": "not found" }));
    }

    let candidate = if rel.is_empty() {
        root.join("index.html")
    } else {
        root.join(rel)
    };

    // Only serve files that resolve inside the frontend root; anything else
    // (unknown asset, traversal attempt) falls back to the SPA entrypoint.
    let resolved = match std::fs::canonicalize(&candidate) {
        Ok(path) if path.starts_with(root) && path.is_file() => Some(path),
        _ => None,
    };

    let file = match resolved {
        Some(path) => path,
        None => {
            let index = root.join("index.html");
            if index.is_file() {
                index
            } else {
                return HttpResponse::NotFound()
                    .json(serde_json::json!({ "error": "not found" }));
            }
        }
    };

    match std::fs::read(&file) {
        Ok(bytes) => {
            // Keep the browser cache short so new deploys are picked up, but
            // allow long-lived caching for hashed assets under /assets.
            HttpResponse::Ok()
                .content_type(mime_for(&file))
                .body(bytes)
        }
        Err(e) => {
            log::error!("Failed to read static asset {}: {}", file.display(), e);
            HttpResponse::InternalServerError()
                .json(serde_json::json!({ "error": "failed to read asset" }))
        }
    }
}
