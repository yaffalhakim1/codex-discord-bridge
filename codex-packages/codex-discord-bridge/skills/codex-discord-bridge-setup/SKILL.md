---
name: codex-discord-bridge-setup
description: Set up, configure, and troubleshoot the Codex Discord Bridge. Use when the user wants to control Codex CLI from Discord or fix a broken bridge instance.
---

# Codex Discord Bridge Setup

You are helping set up or fix codex-discord-bridge: a Rust serenity bot that
connects Discord to the Codex app-server (JSON-RPC over WebSocket on
127.0.0.1:8837 by default).

## Setup checklist

1. Prerequisites: Rust stable, a Discord bot token (developer portal, message
   content intent enabled), Codex CLI authenticated (`codex login status`).
2. Copy `.env.example` to `.env`, fill `DISCORD_TOKEN`.
3. Copy `bridge.json.example` to `bridge.json`. Key options:
   - `stream.live` (live-stream agent output into one edited Discord message)
   - `approvals.ttlMinutes` (button lifetime for approval cards)
   - `autoThread.enabled` + `autoThread.categoryId` (one Discord thread per
     Codex conversation, titled from the first message)
   - `mirror.*` (echo agent messages / user messages / commands / file changes)
4. `cargo build --release`, then run `target/release/codex-discord-bridge`.
5. Invite the bot with `bot` + `applications.commands` scopes; mention it in a
   channel to start a thread.

## Troubleshooting

- **Bot connects but Codex replies never arrive**: check the app-server spawned
  (`Spawned codex app-server` in the log) and that port 8837 is not held by a
  stale `codex.exe` (`netstat -ano | grep 8837`). Kill stale PIDs and restart.
- **Follow-up messages start new conversations**: verify the Discord thread id
  exists in `data/state.json` `thread_map`. If the mapping is missing, the
  bridge treats the message as new. Never edit `data/state.json` while the
  bridge is running.
- **First reply cropped**: fixed by the streaming poster (accumulated full text
  is re-posted after the message id is known). If you see it, you are running a
  build older than the `poster.rs` commit.
- **Restart hangs**: the supervisor must be bounded (15s readiness timeout,
  500ms to 4s capped backoff). Unbounded restart loops mean an old build.

## Development rules

- Tests live in `tests/` and MUST use temp dirs (state path injection via
  `BridgeState::with_path`); a guard test asserts production `data/state.json`
  is never touched by tests.
- Every bug fix ships with a regression test (repo AGENTS.md policy).
- `cargo clippy --all-targets -- -D warnings` must stay clean.
