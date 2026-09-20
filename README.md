# codex-discord-bridge

A 14MB Rust binary that connects [Codex](https://github.com/openai/codex) to Discord. You monitor Codex work, approve commands and file edits, and send messages back to Codex from your phone.

## How it works

The bridge spawns `codex app-server --listen ws://127.0.0.1:8837` as a child process, connects over WebSocket, and speaks JSON-RPC. Codex replies mirror into the Discord channel you used. When Codex needs your permission to run something, you get a card with Approve and Reject buttons.

## Setup

### Step 1: Build

```bash
cargo build --release
cp .env.example .env
```

### Step 2: Create the Discord bot

1. Open the [Discord Developer Portal](https://discord.com/developers/applications).
2. Click **New Application**. Name it (for example, `Codex Bridge`).
3. Go to the **Bot** page in the left sidebar.
4. Click **Reset Token**, then copy the token. This is `DISCORD_BOT_TOKEN`.
5. On the same page, scroll down to **Privileged Gateway Intents** and enable **Message Content Intent**. The bridge needs this to read your messages.

### Step 3: Get your IDs

Enable Developer Mode first: in Discord, go to **User Settings → Advanced → Developer Mode** and turn it on. You only need to do this once. With Developer Mode on, right-clicking anything shows a **Copy ID** option.

| Variable | How to get it |
|---|---|
| `DISCORD_APPLICATION_ID` | Developer Portal → your app → **General Information** → copy **Application ID** |
| `DISCORD_GUILD_ID` | In Discord, right-click your **server name** → **Copy Server ID** |
| `DISCORD_CONTROLLER_USER_ID` | In Discord, right-click **your own username** → **Copy User ID** |

Only the user ID you put in `DISCORD_CONTROLLER_USER_ID` can talk to the bot. Everyone else is ignored.

### Step 4: Invite the bot to your server

Replace `<APP_ID>` with your Application ID, then open this URL in a browser:

```
https://discord.com/oauth2/authorize?client_id=<APP_ID>&scope=bot%20applications.commands&permissions=2147551296
```

Choose your server and approve. The bot should appear in the member list.

### Step 5: Fill in `.env`

Open `.env` and fill in the four Discord values you collected:

```env
DISCORD_BOT_TOKEN=your-bot-token-from-step-2
DISCORD_APPLICATION_ID=1234567890123456789
DISCORD_GUILD_ID=9876543210987654321
DISCORD_CONTROLLER_USER_ID=1122334455667788990
CODEX_COMMAND=codex
CODEX_APP_SERVER_LISTEN_URL=ws://127.0.0.1:8837
RUST_LOG=info
```

The last three lines usually don't need changing.

### Step 6: Run

```bash
cargo run --release
```

You should see:

```
INFO codex_discord_bridge: Spawned codex app-server
INFO codex_discord_bridge: Codex client ready.
INFO codex_discord_bridge::discord: Discord bot ready as Codex Bridge
```

## Talking to Codex

You don't need slash commands for normal conversation.

**@mention the bot** in any server channel. Your first message starts a new Codex thread mapped to that channel.

**Reply to a bot message.** Your reply continues that same thread.

**DM the bot.** Each DM channel gets its own Codex thread.

The bridge stays quiet when you send a message. Codex's reply appears when it's ready. If something breaks, the bot tells you what went wrong.

## Slash commands

| Command | What it does |
|---|---|
| `/codex attach <thread_id>` | Maps an existing Codex thread to the current channel |
| `/codex detach <thread_id>` | Removes the mapping |
| `/codex send <text>` | Sends a message to the mapped thread (queues if Codex is busy) |
| `/codex new <prompt>` | Starts a new Codex thread. The bridge owns it, so you get full control |
| `/codex model` | Lists available models. Marks the default with ⭐ |
| `/codex model set:<model_id>` | Sets the model for new threads. Existing threads keep theirs |
| `/codex status` | Lists mapped threads |
| `/codex retract` | Pulls back the last queued message |

## Running it

The bridge must run on the same machine as Codex. Your PC does the work; Discord is the remote control. Keep the bridge running while you're away, and it forwards everything between Discord and Codex.

## Resource usage

One 14MB binary. The only other process is `codex app-server` at 80 to 100MB, which Codex itself spawns.

## Roadmap

Ordered by implementation priority.

### Phase 1: Usability

1. **Persist thread mappings** — save the channel-to-thread map to `data/state.json` so restarting the bridge doesn't lose your mappings.
2. **`/codex stop`** — interrupt the running turn on the mapped thread (`turn/interrupt`).
3. **`/codex threads`** — list recent Codex threads with IDs so you can `/codex attach` without digging through logs.

### Phase 2: Better interaction

4. **Steering** — `/codex send mode:steer` redirects the active turn instead of queueing (`turn/steer`).
5. **Approval timeouts** — expire approval cards after 30 minutes and disable the buttons.
6. **Live streaming** — buffer agent message deltas and edit one Discord message as Codex types.

### Phase 3: Polish

7. **Config file** — `bridge.toml` to toggle what mirrors (file edits, reasoning, command output).
8. **Auto-created threads** — one Discord thread per Codex thread under a category.
9. **Image attachments** — send screenshots from your phone; the protocol accepts `input_image`.
