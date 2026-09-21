use codex_discord_bridge::poster::{dispatch_outbound, finish_stream, handle_stream_delta};
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

impl Default for StreamBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamBuffer {
    pub fn new() -> Self {
        Self {
            text: String::new(),
            dirty: false,
            last_flush: None,
            debounce: Duration::from_millis(1500),
            max_len: 1900,
        }
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
        let elapsed_ok = self
            .last_flush
            .map(|t| now.duration_since(t) >= self.debounce)
            .unwrap_or(true);
        // Force flush when we're near Discord's limit even if debounce hasn't passed
        let near_limit = self.text.len() >= self.max_len;
        if elapsed_ok || near_limit {
            let text = std::mem::take(&mut self.text);
            self.dirty = false;
            self.last_flush = Some(now);
            let _is_full = text.len() >= self.max_len;
            Flush::Edit(text)
        } else {
            Flush::Idle
        }
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty() && !self.dirty
    }
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
    if let Flush::Edit(t) = b.maybe_flush(Instant::now()) {
        full.push_str(&t);
    }
    b.push(" world");
    let later = Instant::now() + Duration::from_secs(2);
    if let Flush::Edit(t) = b.maybe_flush(later) {
        full.push_str(&t);
    }
    assert_eq!(full, "Hello world");
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PosterAction {
    Start(u64, String),
    Edit(u64, u64, String),
}

#[derive(Debug)]
struct RecordingPoster {
    actions: std::sync::Arc<tokio::sync::Mutex<Vec<PosterAction>>>,
    next_message_id: std::sync::atomic::AtomicU64,
}

#[async_trait::async_trait]
impl codex_discord_bridge::poster::DiscordPoster for RecordingPoster {
    async fn send_plain(&self, _channel_id: u64, _content: String) -> Result<(), serenity::Error> {
        unreachable!("plain messages are unrelated to the stream regression")
    }

    async fn send_start_stream(
        &self,
        channel_id: u64,
        content: String,
    ) -> Result<serenity::all::MessageId, serenity::Error> {
        self.actions
            .lock()
            .await
            .push(PosterAction::Start(channel_id, content));
        let message_id = self
            .next_message_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            + 1;
        Ok(serenity::all::MessageId::new(message_id))
    }

    async fn edit_stream(
        &self,
        channel_id: u64,
        message_id: u64,
        content: String,
    ) -> Result<(), serenity::Error> {
        self.actions
            .lock()
            .await
            .push(PosterAction::Edit(channel_id, message_id, content));
        Ok(())
    }

    async fn create_thread(
        &self,
        _category_id: u64,
        _name: String,
    ) -> Result<u64, serenity::Error> {
        unreachable!("thread creation is unrelated to the stream regression")
    }

    async fn send_approval_card(
        &self,
        _channel_id: u64,
        _title: String,
        _detail: String,
        _token: String,
    ) -> Result<(), serenity::Error> {
        unreachable!("approval cards are unrelated to the stream regression")
    }
}

#[tokio::test]
async fn first_turn_stream_edits_the_same_accumulated_message() {
    let (event_tx, _event_rx) = tokio::sync::mpsc::unbounded_channel();
    let temp = tempfile::TempDir::new().unwrap();
    let state: std::sync::Arc<codex_discord_bridge::state::BridgeState> =
        codex_discord_bridge::state::BridgeState::with_path(
            event_tx,
            temp.path().join("state.json"),
        );
    state.map_thread("stream-thread", 900_001);

    let actions = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let poster = RecordingPoster {
        actions: actions.clone(),
        next_message_id: std::sync::atomic::AtomicU64::new(101),
    };
    let (outbound_tx, mut outbound_rx) = tokio::sync::mpsc::unbounded_channel();
    let debounce = std::time::Duration::ZERO;

    // Codex sends the response in fragments. The first fragment starts a
    // Discord message; the later fragments must edit that same message.
    handle_stream_delta("stream-thread", "He", &state, &outbound_tx, true, debounce);
    let outbound = outbound_rx.recv().await.expect("first stream flush");
    let dispatch = dispatch_outbound(outbound, &state, &poster);
    tokio::time::timeout(std::time::Duration::from_secs(2), dispatch)
        .await
        .expect("dispatch timed out");

    assert_eq!(
        *actions.lock().await,
        vec![PosterAction::Start(900_001, "He".to_string())]
    );

    handle_stream_delta(
        "stream-thread",
        "i what can i do for you",
        &state,
        &outbound_tx,
        true,
        debounce,
    );
    let later_outbound = outbound_rx.recv().await.expect("later stream flush");
    let dispatch = dispatch_outbound(later_outbound, &state, &poster);
    tokio::time::timeout(std::time::Duration::from_secs(2), dispatch)
        .await
        .expect("dispatch timed out");
    assert_eq!(
        *actions.lock().await,
        vec![
            PosterAction::Start(900_001, "He".to_string()),
            PosterAction::Edit(900_001, 102, "Hei what can i do for you".to_string()),
        ]
    );
    assert_eq!(
        state
            .streams
            .read()
            .await
            .get("stream-thread")
            .unwrap()
            .full_text,
        "Hei what can i do for you"
    );

    // The turn finalizer must not regress the message back to a fragment.
    finish_stream("stream-thread", &state, &outbound_tx, true);
    assert!(outbound_rx.try_recv().is_err());
    assert_eq!(actions.lock().await.len(), 2);
}
#[tokio::test]
async fn late_delta_while_first_start_is_in_flight_is_captured_into_same_message() {
    let (event_tx, _event_rx) = tokio::sync::mpsc::unbounded_channel();
    let temp = tempfile::TempDir::new().unwrap();
    let state: std::sync::Arc<codex_discord_bridge::state::BridgeState> =
        codex_discord_bridge::state::BridgeState::with_path(
            event_tx,
            temp.path().join("state.json"),
        );
    state.map_thread("stream-start-race", 900_002);

    let (outbound_tx, mut outbound_rx) = tokio::sync::mpsc::unbounded_channel();
    let debounce = std::time::Duration::ZERO;

    handle_stream_delta(
        "stream-start-race",
        "He",
        &state,
        &outbound_tx,
        true,
        debounce,
    );
    let first_outbound = outbound_rx.recv().await.expect("first stream flush");

    // The POST has not returned yet (no discord_message_id), but another
    // debounce expires. It must not start a second Discord message.
    handle_stream_delta(
        "stream-start-race",
        "i what can i do for you",
        &state,
        &outbound_tx,
        true,
        debounce,
    );
    assert!(outbound_rx.try_recv().is_err(),);

    let actions = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let poster = RecordingPoster {
        actions: actions.clone(),
        next_message_id: std::sync::atomic::AtomicU64::new(101),
    };
    dispatch_outbound(first_outbound, &state, &poster).await;

    assert_eq!(
        *actions.lock().await,
        vec![
            PosterAction::Start(900_002, "He".to_string()),
            PosterAction::Edit(900_002, 102, "Hei what can i do for you".to_string()),
        ],
        "the poster must catch the late delta up into the first message"
    );
    assert_eq!(
        state
            .streams
            .read()
            .await
            .get("stream-start-race")
            .unwrap()
            .full_text,
        "Hei what can i do for you"
    );
}
