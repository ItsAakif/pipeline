use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use actix_web::body::{BoxBody, EitherBody, MessageBody};
use actix_web::dev::{forward_ready, Service, ServiceRequest, ServiceResponse, Transform};
use actix_web::{http::header, Error, HttpResponse};
use futures::future::{ready, LocalBoxFuture, Ready};

/// Parse the client IP from a forwarded header value.
///
/// `X-Forwarded-For` is a comma-separated list (`client, proxy1, proxy2`); the
/// first entry is the original client.
fn parse_forwarded(headers: &header::HeaderMap, name: &str) -> Option<IpAddr> {
    let value = headers.get(name)?.to_str().ok()?;
    let first = value.split(',').next()?.trim();
    first.parse::<IpAddr>().ok()
}

/// Resolve the client IP for abuse controls.
///
/// When `trust_proxy` is true, prefer the first `X-Forwarded-For` entry, then
/// `X-Real-IP`, then the direct peer address. When false, always use the peer
/// address (the forwarded headers are spoofable, so they must only be trusted
/// behind a proxy that overwrites them).
pub fn client_ip(
    headers: &header::HeaderMap,
    peer_addr: Option<std::net::SocketAddr>,
    trust_proxy: bool,
) -> IpAddr {
    if trust_proxy {
        if let Some(ip) = parse_forwarded(headers, "x-forwarded-for") {
            return ip;
        }
        if let Some(ip) = parse_forwarded(headers, "x-real-ip") {
            return ip;
        }
    }

    peer_addr
        .map(|addr| addr.ip())
        .unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED))
}

/// Per-IP sliding-window rate limiter for the `/api/*` surface (R7).
///
/// A limit of `0` disables limiting entirely. Kept deliberately small: a single
/// mutex-guarded map of recent request timestamps per IP.
pub struct RateLimiter {
    limit: usize,
    window: Duration,
    hits: Mutex<HashMap<IpAddr, Vec<Instant>>>,
}

impl RateLimiter {
    pub fn new(limit_per_minute: usize) -> Self {
        Self {
            limit: limit_per_minute,
            window: Duration::from_secs(60),
            hits: Mutex::new(HashMap::new()),
        }
    }

    pub fn enabled(&self) -> bool {
        self.limit > 0
    }

    /// Returns `Ok(())` when the request is allowed, or `Err(retry_after_secs)`
    /// when the caller has exhausted its budget for the current window.
    pub fn check(&self, ip: IpAddr) -> Result<(), u64> {
        if !self.enabled() {
            return Ok(());
        }

        let now = Instant::now();
        let mut hits = self.hits.lock().unwrap();

        // Bound memory if the process sees a large number of distinct IPs.
        if hits.len() > 10_000 {
            hits.retain(|_, times| {
                times.iter().any(|t| now.duration_since(*t) < self.window)
            });
        }

        let entry = hits.entry(ip).or_default();
        entry.retain(|t| now.duration_since(*t) < self.window);

        if entry.len() >= self.limit {
            let oldest = entry.iter().min().copied().unwrap_or(now);
            let retry = self
                .window
                .saturating_sub(now.duration_since(oldest))
                .as_secs()
                .max(1);
            return Err(retry);
        }

        entry.push(now);
        Ok(())
    }
}

/// Middleware that enforces [`RateLimiter`] on `/api/*` (excluding health).
pub struct RateLimit {
    limiter: Arc<RateLimiter>,
    trust_proxy: bool,
}

impl RateLimit {
    pub fn new(limiter: Arc<RateLimiter>, trust_proxy: bool) -> Self {
        Self {
            limiter,
            trust_proxy,
        }
    }
}

impl<S, B> Transform<S, ServiceRequest> for RateLimit
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error> + 'static,
    S::Future: 'static,
    B: MessageBody + 'static,
{
    type Response = ServiceResponse<EitherBody<BoxBody, B>>;
    type Error = Error;
    type Transform = RateLimitMiddleware<S>;
    type InitError = ();
    type Future = Ready<Result<Self::Transform, Self::InitError>>;

    fn new_transform(&self, service: S) -> Self::Future {
        ready(Ok(RateLimitMiddleware {
            service,
            limiter: self.limiter.clone(),
            trust_proxy: self.trust_proxy,
        }))
    }
}

pub struct RateLimitMiddleware<S> {
    service: S,
    limiter: Arc<RateLimiter>,
    trust_proxy: bool,
}

impl<S, B> Service<ServiceRequest> for RateLimitMiddleware<S>
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error> + 'static,
    S::Future: 'static,
    B: MessageBody + 'static,
{
    type Response = ServiceResponse<EitherBody<BoxBody, B>>;
    type Error = Error;
    type Future = LocalBoxFuture<'static, Result<Self::Response, Self::Error>>;

    forward_ready!(service);

    fn call(&self, req: ServiceRequest) -> Self::Future {
        let path = req.path();
        let should_limit = path.starts_with("/api/") && path != "/api/health";

        if !should_limit {
            let fut = self.service.call(req);
            return Box::pin(async move {
                fut.await.map(|res| res.map_into_right_body::<BoxBody>())
            });
        }

        let ip = client_ip(req.headers(), req.peer_addr(), self.trust_proxy);

        match self.limiter.check(ip) {
            Ok(()) => {
                let fut = self.service.call(req);
                Box::pin(async move {
                    fut.await.map(|res| res.map_into_right_body::<BoxBody>())
                })
            }
            Err(retry_after) => Box::pin(async move {
                let response = HttpResponse::TooManyRequests()
                    .insert_header((header::RETRY_AFTER, retry_after.to_string()))
                    .json(serde_json::json!({
                        "error": "rate limit exceeded",
                        "retry_after_secs": retry_after,
                    }));
                Ok(req.into_response(response).map_into_left_body::<B>())
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddr;

    fn headers(pairs: &[(&str, &str)]) -> header::HeaderMap {
        let mut map = header::HeaderMap::new();
        for (name, value) in pairs {
            map.insert(
                header::HeaderName::from_bytes(name.as_bytes()).unwrap(),
                header::HeaderValue::from_str(value).unwrap(),
            );
        }
        map
    }

    fn peer(addr: &str) -> Option<SocketAddr> {
        Some(addr.parse().unwrap())
    }

    #[test]
    fn client_ip_prefers_first_forwarded_for_entry() {
        let h = headers(&[("x-forwarded-for", "203.0.113.7, 10.0.0.1, 10.0.0.2")]);
        assert_eq!(
            client_ip(&h, peer("10.0.0.1:5000"), true),
            "203.0.113.7".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn client_ip_falls_back_to_x_real_ip() {
        let h = headers(&[("x-real-ip", "198.51.100.9")]);
        assert_eq!(
            client_ip(&h, peer("10.0.0.1:5000"), true),
            "198.51.100.9".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn client_ip_falls_back_to_peer() {
        let h = headers(&[]);
        assert_eq!(
            client_ip(&h, peer("10.0.0.1:5000"), true),
            "10.0.0.1".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn client_ip_ignores_forwarded_headers_when_not_trusting_proxy() {
        let h = headers(&[
            ("x-forwarded-for", "203.0.113.7"),
            ("x-real-ip", "198.51.100.9"),
        ]);
        assert_eq!(
            client_ip(&h, peer("10.0.0.1:5000"), false),
            "10.0.0.1".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn client_ip_ignores_unparseable_forwarded_for() {
        let h = headers(&[("x-forwarded-for", "not-an-ip"), ("x-real-ip", "198.51.100.9")]);
        assert_eq!(
            client_ip(&h, peer("10.0.0.1:5000"), true),
            "198.51.100.9".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn blocks_after_limit_within_window() {
        let limiter = RateLimiter::new(2);
        let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);

        assert!(limiter.check(ip).is_ok(), "first request allowed");
        assert!(limiter.check(ip).is_ok(), "second request allowed");
        assert!(limiter.check(ip).is_err(), "third request rejected");
    }

    #[test]
    fn disabled_limit_allows_everything() {
        let limiter = RateLimiter::new(0);
        let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);

        for _ in 0..1000 {
            assert!(limiter.check(ip).is_ok());
        }
    }

    #[test]
    fn limits_are_per_ip() {
        let limiter = RateLimiter::new(1);
        let a = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        let b = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2));

        assert!(limiter.check(a).is_ok());
        assert!(limiter.check(a).is_err());
        assert!(limiter.check(b).is_ok(), "different IP has its own budget");
    }
}
