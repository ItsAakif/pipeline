//! Per-IP cooldown after an accepted `POST /api/run`.
//!
//! Compiling and running a program is expensive, so after a run is accepted for
//! an IP, further runs from that IP are rejected for `RUN_COOLDOWN_SECS` (default
//! 180; `0` disables). Memory stays bounded by pruning stale entries, like the
//! rate limiter.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub struct Cooldown {
    window: Duration,
    last: Mutex<HashMap<IpAddr, Instant>>,
}

impl Cooldown {
    /// `secs == 0` disables the cooldown.
    pub fn new(secs: u64) -> Self {
        Self {
            window: Duration::from_secs(secs),
            last: Mutex::new(HashMap::new()),
        }
    }

    pub fn enabled(&self) -> bool {
        !self.window.is_zero()
    }

    /// If the IP is allowed to run, records this run and returns `Ok(())`.
    /// Otherwise returns `Err(retry_after_secs)` without recording.
    pub fn acquire(&self, ip: IpAddr) -> Result<(), u64> {
        if !self.enabled() {
            return Ok(());
        }

        let now = Instant::now();
        let mut last = self.last.lock().unwrap();

        // Bound memory if the process sees many distinct IPs.
        if last.len() > 10_000 {
            last.retain(|_, recorded| now.duration_since(*recorded) < self.window);
        }

        if let Some(recorded) = last.get(&ip) {
            let elapsed = now.duration_since(*recorded);
            if elapsed < self.window {
                let retry_after = (self.window - elapsed).as_secs().max(1);
                return Err(retry_after);
            }
        }

        last.insert(ip, now);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn cooldown_blocks_then_expires() {
        let cooldown = Cooldown::new(1);
        let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);

        assert!(cooldown.acquire(ip).is_ok(), "first run allowed");
        let retry = cooldown.acquire(ip).expect_err("second run blocked");
        assert!(retry >= 1, "retry_after must be at least 1s");

        std::thread::sleep(Duration::from_millis(1100));
        assert!(cooldown.acquire(ip).is_ok(), "allowed again after the window");
    }

    #[test]
    fn cooldown_zero_disables() {
        let cooldown = Cooldown::new(0);
        let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);

        for _ in 0..1000 {
            assert!(cooldown.acquire(ip).is_ok(), "0 disables the cooldown");
        }
    }

    #[test]
    fn cooldown_is_per_ip() {
        let cooldown = Cooldown::new(60);
        let a = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        let b = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2));

        assert!(cooldown.acquire(a).is_ok());
        assert!(cooldown.acquire(a).is_err(), "same IP is blocked");
        assert!(cooldown.acquire(b).is_ok(), "different IP is not blocked");
    }
}
