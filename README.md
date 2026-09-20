# codex-discord-bridge

A 14MB Rust binary that connects [Codex](https://github.com/openai/codex) to Discord. You monitor Codex work, approve commands and file edits, and send messages back to Codex from your phone.

## How it works

The bridge spawns `codex app-server --listen ws://127.0.0.1:8837` as a child process, connects over WebSocket, and speaks JSON-RPC. Codex activity (agent messages, commands, approvals) mirrors into mapped Discord channels. When Codex needs your permission to run something, you get a card with Approve and Reject buttons.

## Setup

```bash
cargo build --release
cp .env.example .env
# Fill in your Discord credentials in .env
cargo run --release
```

Required bot permissions: `View Channels`, `Send Messages`, `Send Messages in Threads`, `Read Message History`.

## Talking to Codex

You don't need slash commands for normal conversation.

**@mention the bot** in any server channel. Your first message starts a new Codex thread mapped to that channel.

**Reply to a bot message.** Your reply continues that same thread.

**DM the bot.** Each DM channel gets its own Codex thread.

Codex responses mirror back into the channel you used.

## Slash commands

| Command | What it does |
|---|---|
| `/codex attach <thread_id>` | Maps an existing Codex thread to the current channel |
| `/codex detach <thread_id>` | Removes the mapping |
| `/codex send <text>` | Sends a message to the mapped thread (queues if Codex is busy) |
| `/codex new <prompt>` | Starts a new Codex thread. The bridge owns it, so you get full control |
| `/codex status` | Lists mapped threads |
| `/codex retract` | Pulls back the last queued message |

## Resource usage

One 14MB binary. The only other process is `codex app-server` at 80 to 100MB, which Codex itself spawns.