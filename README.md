# codex-discord-bridge

A lightweight Rust bridge that connects [Codex](https://github.com/openai/codex) to Discord.

Monitor Codex work, approve/reject commands and file edits, and send messages back to Codex — all from Discord.

## Architecture

- Runs `codex app-server --listen ws://127.0.0.1:8837` as a child process
- Connects via WebSocket and speaks JSON-RPC to Codex
- Mirrors Codex activity (messages, commands, approvals) to Discord channels
- Renders approval cards with Approve/Reject buttons
- Supports `/codex send` to talk back to Codex

## Setup

1. `cargo build --release`
2. Copy `.env.example` to `.env` and fill in your Discord credentials
3. `cargo run --release`

### Discord Bot Permissions

Required bot permissions: `View Channels`, `Send Messages`, `Send Messages in Threads`, `Read Message History`.

## Usage

- `/codex attach <thread_id>` — map a Codex thread to the current Discord channel
- `/codex detach <thread_id>` — remove the mapping
- `/codex send <text>` — send a message to the mapped Codex thread (queues if busy)
- /codex new <prompt> — start a new Codex thread (the bridge owns it for full control)
- /codex status — list mapped threads
- `/codex retract` — retract the latest queued message

When Codex needs approval for a command or file edit, a card appears in Discord with Approve/Reject buttons.

## Resource Usage

Single ~14MB binary. The only other process is `codex app-server` (~80-100MB), which Codex itself spawns.