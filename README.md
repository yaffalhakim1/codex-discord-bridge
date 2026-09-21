# Codex Discord Bridge

Control Codex CLI from Discord. A Rust (serenity) bridge that connects a
Discord bot to the Codex app-server over WebSocket, so your whole dev loop
runs in a Discord server you can reach from your phone.

```
Discord thread  <->  bridge (serenity bot)  <->  JSON-RPC over WebSocket  <->  codex app-server
```

## What it does

- Mention the bot in a channel: it starts a Codex thread and mirrors output
  back live (streamed into one Discord message per turn).
- Every Discord thread maps 1:1 to a Codex conversation; follow-up messages
  in the thread continue the same conversation.
- Approval requests (command exec, patches) arrive as Discord buttons with a
  TTL, so Codex can ask permission mid-task while you are away from the desk.
- Autothread mode: each conversation gets its own Discord thread under a
  category you pick, titled from the first message.
- Survives disconnects: 20s JSON-RPC keepalive, bounded supervisor with
  capped backoff that reconnects the app-server without dropping your
  Discord session or thread mappings.

## Requirements

- Rust toolchain (stable)
- A Discord application + bot token (developer portal)
- Codex CLI installed and authenticated (`codex` on PATH)

## Setup

1. Clone this repo and copy `.env.example` to `.env`:

   ```
   DISCORD_TOKEN=your-bot-token
   ```

2. Copy `bridge.json.example` to `bridge.json` and adjust:

   ```json
   {
     "mirror": { "agentMessages": true, "userMessages": false, "commands": false, "fileChanges": false },
     "stream": { "live": true },
     "approvals": { "ttlMinutes": 30 },
     "autoThread": { "enabled": true, "categoryId": 123456789012345678 }
   }
   ```

3. Build and run:

   ```
   cargo build --release
   ./target/release/codex-discord-bridge
   ```

4. Invite the bot to your server, mention it in a channel, and code from bed.

## Plugin manifest

This repository ships a Codex plugin manifest at
`.codex-plugin/plugin.json`, so it can be referenced by a Codex plugin
marketplace or installed locally as a plugin package. The manifest
describes the bridge and its setup skill; the bridge binary itself is built
from this repo with cargo.

## Stability notes

- Tests: `cargo test` (98+ tests, including reconnect and routing regression
  suites that run in temp dirs and never touch production state).
- The supervisor reuses an already-listening app-server when safe and only
  kills children it owns.

## License

MIT
