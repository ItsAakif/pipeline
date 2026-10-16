//! Shared request-level abuse-control configuration.

/// Values resolved once at startup and shared across workers.
pub struct ApiConfig {
    /// `TRUST_PROXY` — when true, the client IP comes from `X-Forwarded-For`
    /// (falling back to `X-Real-IP`) instead of the direct peer address.
    ///
    /// Must be `false` when the app is exposed directly: those headers are
    /// spoofable.
    pub trust_proxy: bool,
    /// `MAX_SOURCE_BYTES` — reject `source_code` larger than this (bytes).
    pub max_source_bytes: usize,
}
