use std::time::{Duration, Instant};

/// TDD for live streaming: buffer deltas, debounce edits, chunk at Discord's limit.

pub struct StreamBuffer {
    text: String,
    dirty: bool,
    last_flush: Option<Instant>,
    debounce: Duration,
    max_len: usize,
}

pub enum Flush {
    /// (text, continue_streaming) — edit the message with this text
    Edit(String),
    /// Buffer has nothing new
    Idle,
}

impl StreamBuffer {
    pub fn new() -> Self {
        Self { text: String::new(), dirty: false, last_flush: None, debounce: Duration::from_millis(1500), max_len: 1900 }
    }

    pub fn push(&mut self, delta: &str) {
        self.text.push_str(delta);
        self.dirty = true;
    }

    /// Returns Some((text, is_full)) when it's time to flush.
    pub fn maybe_flush(&mut self, now: Instant) -> Flush {
        if !self.dirty {
            return Flush::Idle;
        }
        let elapsed_ok = self.last_flush.map(|t| now.duration_since(t) >= self.debounce).unwrap_or(true);
        // Force flush when we're near Discord's limit even if debounce hasn't passed
        let near_limit = self.text.len() >= self.max_len;
        if elapsed_ok || near_limit {
            let text = std::mem::take(&mut self.text);
            self.dirty = false;
            self.last_flush = Some(now);
            let is_full = text.len() >= self.max_len;
            Flush::Edit(text)
        } else {
            Flush::Idle
        }
    }

    pub fn is_empty(&self) -> bool { self.text.is_empty() && !self.dirty }
}

#[test]
fn first_delta_flushes_immediately() {
    let mut b = StreamBuffer::new();
    b.push("Hello");
    let now = Instant::now();
    match b.maybe_flush(now) {
        Flush::Edit(t) => assert_eq!(t, "Hello"),
        Flush::Idle => panic!("should flush"),
    }
}

#[test]
fn second_delta_within_debounce_is_idle() {
    let mut b = StreamBuffer::new();
    b.push("Hello");
    let t0 = Instant::now();
    let _ = b.maybe_flush(t0);
    b.push(" world");
    let t1 = t0 + Duration::from_millis(200);
    assert!(matches!(b.maybe_flush(t1), Flush::Idle));
}

#[test]
fn delta_after_debounce_flushes() {
    let mut b = StreamBuffer::new();
    b.push("Hello");
    let t0 = Instant::now();
    let _ = b.maybe_flush(t0);
    b.push(" world");
    let t1 = t0 + Duration::from_millis(1600);
    match b.maybe_flush(t1) {
        Flush::Edit(t) => assert_eq!(t, " world"),
        Flush::Idle => panic!("should flush"),
    }
}

#[test]
fn near_limit_forces_flush_even_within_debounce() {
    let mut b = StreamBuffer::new();
    b.push("Hello");
    let t0 = Instant::now();
    let _ = b.maybe_flush(t0);
    // Push a huge delta exceeding 1900 chars
    let big = "x".repeat(2000);
    b.push(&big);
    let t1 = t0 + Duration::from_millis(100);
    match b.maybe_flush(t1) {
        Flush::Edit(t) => assert!(t.len() >= 1900),
        Flush::Idle => panic!("near-limit should force flush"),
    }
}

#[test]
fn flush_empties_buffer() {
    let mut b = StreamBuffer::new();
    b.push("data");
    let _ = b.maybe_flush(Instant::now());
    assert!(b.is_empty());
}

#[test]
fn idle_when_no_deltas() {
    let mut b = StreamBuffer::new();
    assert!(matches!(b.maybe_flush(Instant::now()), Flush::Idle));
}

#[test]
fn final_edits_accumulate_into_full_message() {
    // Simulate: flush1 "Hello", flush2 " world" -> final message text is concatenation
    let mut full = String::new();
    let mut b = StreamBuffer::new();
    b.push("Hello");
    if let Flush::Edit(t) = b.maybe_flush(Instant::now()) { full.push_str(&t); }
    b.push(" world");
    let later = Instant::now() + Duration::from_secs(2);
    if let Flush::Edit(t) = b.maybe_flush(later) { full.push_str(&t); }
    assert_eq!(full, "Hello world");
}