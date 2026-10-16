//! Container-aware memory-limit detection.
//!
//! Plain `std::fs` reads only (no crates). Priority order:
//!   1. cgroup v2 `/sys/fs/cgroup/memory.max`
//!   2. cgroup v1 `/sys/fs/cgroup/memory/memory.limit_in_bytes`
//!   3. `/proc/meminfo` `MemTotal:`
//!
//! `None` means the limit could not be determined (treat as unlimited).

use std::fs;

/// Limits at or above this are treated as "no effective limit". cgroup v1
/// reports a huge sentinel (e.g. `9223372036854771712`) when the cgroup is
/// effectively unlimited.
const UNLIMITED_THRESHOLD_BYTES: u64 = 1 << 60; // 1 EiB

/// Parse the contents of a cgroup memory-limit file.
///
/// Returns `None` for `max`, empty/zero, unparseable, or sentinel (unlimited)
/// values.
fn parse_cgroup_limit(contents: &str) -> Option<u64> {
    let trimmed = contents.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("max") {
        return None;
    }

    let value: u64 = trimmed.parse().ok()?;
    if value == 0 || value >= UNLIMITED_THRESHOLD_BYTES {
        return None;
    }

    Some(value)
}

/// Parse `/proc/meminfo` and return `MemTotal` in bytes.
fn parse_meminfo_total(contents: &str) -> Option<u64> {
    for line in contents.lines() {
        if let Some(rest) = line.strip_prefix("MemTotal:") {
            let kb: u64 = rest.split_whitespace().next()?.parse().ok()?;
            if kb == 0 {
                return None;
            }
            return kb.checked_mul(1024);
        }
    }

    None
}

/// Detect the effective memory limit in bytes.
///
/// `None` means unlimited / unknown (e.g. a bare-metal host with no cgroup
/// limit, or a platform where none of the sources are readable).
pub fn detect_memory_limit() -> Option<u64> {
    // 1. cgroup v2
    if let Ok(contents) = fs::read_to_string("/sys/fs/cgroup/memory.max") {
        if let Some(limit) = parse_cgroup_limit(&contents) {
            return Some(limit);
        }
    }

    // 2. cgroup v1
    if let Ok(contents) = fs::read_to_string("/sys/fs/cgroup/memory/memory.limit_in_bytes") {
        if let Some(limit) = parse_cgroup_limit(&contents) {
            return Some(limit);
        }
    }

    // 3. /proc/meminfo
    if let Ok(contents) = fs::read_to_string("/proc/meminfo") {
        if let Some(total) = parse_meminfo_total(&contents) {
            return Some(total);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_numeric_cgroup_limit() {
        assert_eq!(parse_cgroup_limit("536870912\n"), Some(536_870_912));
    }

    #[test]
    fn treats_cgroup_v2_max_as_unlimited() {
        assert_eq!(parse_cgroup_limit("max\n"), None);
    }

    #[test]
    fn treats_cgroup_v1_sentinel_as_unlimited() {
        assert_eq!(parse_cgroup_limit("9223372036854771712"), None);
    }

    #[test]
    fn treats_zero_and_garbage_as_unlimited() {
        assert_eq!(parse_cgroup_limit("0"), None);
        assert_eq!(parse_cgroup_limit("not-a-number"), None);
        assert_eq!(parse_cgroup_limit("   "), None);
    }

    #[test]
    fn parses_meminfo_total_as_bytes() {
        let sample = "MemTotal:       16384000 kB\nMemFree:         100 kB\nMemAvailable:  200 kB\n";
        assert_eq!(parse_meminfo_total(sample), Some(16_384_000 * 1024));
    }

    #[test]
    fn meminfo_without_total_is_none() {
        assert_eq!(parse_meminfo_total("MemFree: 1 kB\n"), None);
    }
}
