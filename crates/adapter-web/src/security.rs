use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::http::header::{CACHE_CONTROL, HeaderName, HeaderValue, REFERRER_POLICY, X_CONTENT_TYPE_OPTIONS, X_FRAME_OPTIONS};
use axum::http::HeaderMap;

const RATE_WINDOW: Duration = Duration::from_secs(60);
const MAX_REQUESTS_PER_WINDOW: u32 = 120;
const MAX_TRACKED_IPS: usize = 2048;

/// Extracts the real client IP for per-visitor rate limiting without allowing IP spoofing:
/// - ONLY when the TCP peer is loopback (trusted local `cloudflared` tunnel / reverse proxy),
///   honor `CF-Connecting-IP` (or first valid `X-Forwarded-For` hop);
/// - For non-loopback peers, strictly ignore forwarded headers and use `addr.ip()`.
pub fn extract_client_ip(addr: SocketAddr, headers: &HeaderMap) -> IpAddr {
    if !addr.ip().is_loopback() {
        return addr.ip();
    }
    if let Some(ip) = headers
        .get("cf-connecting-ip")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<IpAddr>().ok())
    {
        return ip;
    }
    if let Some(ip) = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .and_then(|s| s.trim().parse::<IpAddr>().ok())
    {
        return ip;
    }
    addr.ip()
}

/// Constant-time byte comparison to prevent timing side-channels on room tokens.
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (&x, &y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[derive(Clone, Default)]
pub struct IpRateLimiter {
    buckets: Arc<Mutex<HashMap<IpAddr, (Instant, u32)>>>,
}

impl IpRateLimiter {
    pub fn allow(&self, ip: IpAddr) -> bool {
        let now = Instant::now();
        let Ok(mut map) = self.buckets.lock() else {
            return true;
        };
        if map.len() >= MAX_TRACKED_IPS && !map.contains_key(&ip) {
            map.retain(|_, (started, _)| now.duration_since(*started) < RATE_WINDOW);
            if map.len() >= MAX_TRACKED_IPS {
                map.clear();
            }
        }
        let entry = map.entry(ip).or_insert((now, 0));
        if now.duration_since(entry.0) >= RATE_WINDOW {
            *entry = (now, 1);
            true
        } else if entry.1 < MAX_REQUESTS_PER_WINDOW {
            entry.1 += 1;
            true
        } else {
            false
        }
    }
}

pub fn security_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    headers.insert(X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        HeaderName::from_static("content-security-policy"),
        HeaderValue::from_static(
            "default-src 'self'; style-src 'unsafe-inline'; script-src 'unsafe-inline'; connect-src 'self'; img-src 'self' data:; object-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'",
        ),
    );
    headers
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use super::*;

    #[test]
    fn constant_time_eq_and_rate_limiter_work() {
        assert!(constant_time_eq(b"tok123", b"tok123"));
        assert!(!constant_time_eq(b"tok123", b"tok124"));
        assert!(!constant_time_eq(b"tok123", b"tok12"));

        let limiter = IpRateLimiter::default();
        let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
        for _ in 0..MAX_REQUESTS_PER_WINDOW {
            assert!(limiter.allow(ip));
        }
        assert!(!limiter.allow(ip));

        // Loopback peer (cloudflared tunnel) honors CF-Connecting-IP; external peer NEVER trusts spoofed headers!
        let mut hdrs = HeaderMap::new();
        hdrs.insert("cf-connecting-ip", HeaderValue::from_static("203.0.113.42"));
        let local_peer = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 50000);
        let public_peer = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(198, 51, 100, 7)), 50000);
        assert_eq!(extract_client_ip(local_peer, &hdrs), IpAddr::V4(Ipv4Addr::new(203, 0, 113, 42)));
        assert_eq!(extract_client_ip(public_peer, &hdrs), IpAddr::V4(Ipv4Addr::new(198, 51, 100, 7)));
    }
}
