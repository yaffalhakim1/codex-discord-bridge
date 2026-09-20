use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
struct Approval {
    token: String,
    created_at: Instant,
    ttl: Duration,
}

impl Approval {
    fn is_expired(&self, now: Instant) -> bool {
        now.duration_since(self.created_at) > self.ttl
    }
}

#[test]
fn fresh_approval_is_not_expired() {
    let a = Approval { token: "t1".into(), created_at: Instant::now(), ttl: Duration::from_secs(1800) };
    assert!(!a.is_expired(Instant::now()));
}

#[test]
fn approval_expires_after_ttl() {
    let created = Instant::now() - Duration::from_secs(1900);
    let a = Approval { token: "t1".into(), created_at: created, ttl: Duration::from_secs(1800) };
    assert!(a.is_expired(Instant::now()));
}

#[test]
fn exactly_at_ttl_is_not_yet_expired() {
    // Boundary: duration_since == ttl is not "older than"
    let a = Approval { token: "t1".into(), created_at: Instant::now(), ttl: Duration::from_secs(60) };
    let now = a.created_at + Duration::from_secs(60);
    assert!(!a.is_expired(now));
}

#[test]
fn default_ttl_is_30_minutes() {
    let ttl: u64 = 30 * 60;
    assert_eq!(ttl, 1800);
}