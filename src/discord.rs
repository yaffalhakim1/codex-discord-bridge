use serenity::all::{
    ButtonStyle, ChannelId, Colour, CreateButton, CreateEmbed, CreateInteractionResponse,
    CreateInteractionResponseMessage, CreateMessage, GuildId, Interaction, Message, MessageId,
    ReactionType,
};
use serenity::async_trait;
use serenity::builder::{CreateActionRow, CreateCommand, CreateCommandOption};
use serenity::client::{Context, EventHandler};
use serenity::http::Http;
use serenity::model::application::{CommandDataOptionValue, CommandInteraction, CommandOptionType, InteractionResponseFlags};
use serenity::model::gateway::Ready;
use tracing::{debug, error, info};

use std::sync::OnceLock;

use crate::config::Config;
use crate::state::SharedState;

pub struct DiscordHandler {
    pub config: Config,
    pub state: SharedState,
    pub bot_user_id: OnceLock<u64>,
}

impl DiscordHandler {
    pub fn new(config: Config, state: SharedState) -> Self {
        Self { config, state, bot_user_id: OnceLock::new() }
    }
}

#[async_trait]
impl EventHandler for DiscordHandler {
    async fn ready(&self, ctx: Context, ready: Ready) {
        info!("Discord bot ready as {}", ready.user.name);
        let _ = self.bot_user_id.set(ready.user.id.get());
        let guild_id = GuildId::new(self.config.discord_guild_id);
        let commands = vec![
            CreateCommand::new("codex")
                .description("Monitor and control Codex from Discord.")
                .add_option(
                    CreateCommandOption::new(CommandOptionType::SubCommand, "status", "List mapped Codex conversations."),
                )
                .add_option(
                    CreateCommandOption::new(CommandOptionType::SubCommand, "send", "Send a message to the mapped Codex thread.")
                        .add_sub_option(
                            CreateCommandOption::new(CommandOptionType::String, "text", "Message for Codex").required(true),
                        ),
                )
                .add_option(
                    CreateCommandOption::new(CommandOptionType::SubCommand, "retract", "Retract the latest pending message."),
                )
                .add_option(
                    CreateCommandOption::new(CommandOptionType::SubCommand, "attach", "Attach to an existing Codex thread.")
                        .add_sub_option(
                            CreateCommandOption::new(CommandOptionType::String, "thread_id", "Codex thread id").required(true),
                        ),
                )
                .add_option(
                    CreateCommandOption::new(CommandOptionType::SubCommand, "detach", "Detach from a mapped Codex thread.")
                        .add_sub_option(
                            CreateCommandOption::new(CommandOptionType::String, "thread_id", "Codex thread id").required(true),
                        ),
                )
                .add_option(
                    CreateCommandOption::new(CommandOptionType::SubCommand, "new", "Start a new Codex thread with an initial prompt.")
                        .add_sub_option(
                            CreateCommandOption::new(CommandOptionType::String, "prompt", "Initial prompt for Codex").required(true),
                        )
                        .add_sub_option(
                            CreateCommandOption::new(CommandOptionType::String, "cwd", "Working directory (optional)"),
                        ),
                )
                .add_option(
                    CreateCommandOption::new(CommandOptionType::SubCommand, "stop", "Interrupt the running Codex turn in this channel."),
                )
                .add_option(
                    CreateCommandOption::new(CommandOptionType::SubCommand, "threads", "List recent Codex threads."),
                )
                .add_option(
                    CreateCommandOption::new(CommandOptionType::SubCommand, "model", "List available models or set one for new threads.")
                        .add_sub_option(
                            CreateCommandOption::new(CommandOptionType::String, "set", "Model ID to use for new threads"),
                        ),
                )
        ];
        match guild_id.set_commands(&ctx.http, commands).await {
            Ok(_) => info!("Slash commands registered."),
            Err(e) => error!("Failed to register slash commands: {e}"),
        }
    }

    async fn interaction_create(&self, ctx: Context, interaction: Interaction) {
        match interaction {
            Interaction::Command(cmd) => self.handle_command(ctx, cmd).await,
            Interaction::Component(comp) => {
                let user_id = comp.user.id.get();
                if user_id != self.config.controller_user_id {
                    let _ = comp
                        .create_response(
                            &ctx.http,
                            CreateInteractionResponse::Message(
                                CreateInteractionResponseMessage::new()
                                    .content("Only the controller can approve or reject.")
                                    .ephemeral(true),
                            ),
                        )
                        .await;
                    return;
                }
                let custom_id = comp.data.custom_id.clone();
                let parts: Vec<&str> = custom_id.split(':').collect();
                if parts.len() < 2 { return; }
                let action = parts[0];
                let token = parts[1];
                match action {
                    "approve" | "decline" | "cancel" => {
                        if let Err(e) = self.state.handle_approval_decision(token, action).await {
                            let _ = comp.create_response(&ctx.http, CreateInteractionResponse::Message(
                                CreateInteractionResponseMessage::new().content(format!("Error: {e}")).ephemeral(true),
                            )).await;
                        } else {
                            let _ = comp.create_response(&ctx.http, CreateInteractionResponse::Acknowledge).await;
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    async fn message(&self, ctx: Context, msg: Message) {
        if msg.author.bot { return; }
        if msg.author.id.get() != self.config.controller_user_id { return; }

        let bot_id = match self.bot_user_id.get() { Some(id) => *id, None => return };
        let is_dm = msg.guild_id.is_none();
        let is_mention = msg.mentions.iter().any(|u| u.id.get() == bot_id);
        let is_reply_to_bot = msg.referenced_message.as_ref()
            .map(|r| r.author.id.get() == bot_id)
            .unwrap_or(false);

        if !is_dm && !is_mention && !is_reply_to_bot { return; }

        // Strip the mention from the content
        let raw = msg.content.clone();
        let text = raw
            .replace(&format!("<@{bot_id}>"), "")
            .replace(&format!("<@!{bot_id}>"), "")
            .trim()
            .to_string();
        if text.is_empty() { return; }


        let channel_id = msg.channel_id.get();
        let mapped = self.state.reverse_map.contains_key(&channel_id);

        // Show typing indicator while Codex works
        let _ = msg.channel_id.broadcast_typing(&ctx.http).await;

        let image_urls: Vec<String> = msg.attachments.iter().map(|a| a.url.clone()).collect();
        // Every free-form message spawns a fresh Codex thread (stateless chat).
        let result = self.state.start_new_thread_in_channel(&channel_id, &text, None).await;


        if let Err(e) = result {
            let _ = msg.reply(&ctx.http, format!("❌ {e}")).await;
        }
    }
}

impl DiscordHandler {
    async fn handle_command(&self, ctx: Context, cmd: CommandInteraction) {
        if cmd.user.id.get() != self.config.controller_user_id {
            let _ = cmd.create_response(&ctx.http, CreateInteractionResponse::Message(
                CreateInteractionResponseMessage::new()
                    .content("Only the controller can use this bot.")
                    .ephemeral(true)
                    .flags(InteractionResponseFlags::EPHEMERAL),
            )).await;
            return;
        }
        let sub = cmd.data.options.first().map(|o| o.name.as_str());
        match sub {
            Some("status") => self.cmd_status(&ctx, &cmd).await,
            Some("send") => self.cmd_send(&ctx, &cmd).await,
            Some("retract") => self.cmd_retract(&ctx, &cmd).await,
            Some("attach") => self.cmd_attach(&ctx, &cmd).await,
            Some("detach") => self.cmd_detach(&ctx, &cmd).await,
            Some("new") => self.cmd_new(&ctx, &cmd).await,
            Some("stop") => self.cmd_stop(&ctx, &cmd).await,
            Some("threads") => self.cmd_threads(&ctx, &cmd).await,
            Some("model") => self.cmd_model(&ctx, &cmd).await,
            _ => {
                let _ = cmd.create_response(&ctx.http, CreateInteractionResponse::Message(
                    CreateInteractionResponseMessage::new().content("Unknown subcommand.").ephemeral(true),
                )).await;
            }
        }
    }

    fn extract_string_option(cmd: &CommandInteraction) -> Option<String> {
        cmd.data.options.first().and_then(|o| {
            if let CommandDataOptionValue::String(s) = &o.value { Some(s.clone()) } else { None }
        })
    }

    async fn cmd_status(&self, ctx: &Context, cmd: &CommandInteraction) {
        let threads = self.state.list_mapped_threads().await;
        let content: String = if threads.is_empty() {
            "No mapped Codex threads yet.".to_string()
        } else {
            let mut s = String::from("**Mapped Codex Threads:**\n");
            for t in threads {
                s.push_str(&format!("• `{}` → <#{}>\n", t.codex_thread_id, t.discord_channel_id));
            }
            s
        };
        let _ = cmd.create_response(&ctx.http, CreateInteractionResponse::Message(
            CreateInteractionResponseMessage::new().content(content).ephemeral(true),
        )).await;
    }

    async fn cmd_send(&self, ctx: &Context, cmd: &CommandInteraction) {
        let text = Self::extract_string_option(cmd).unwrap_or_default();
        if text.is_empty() {
            let _ = cmd.create_response(&ctx.http, CreateInteractionResponse::Message(
                CreateInteractionResponseMessage::new().content("Message cannot be empty.").ephemeral(true),
            )).await;
            return;
        }
        let channel_id = cmd.channel_id.get();
        let mode = cmd.data.options.get(1).and_then(|o| o.value.as_str().map(String::from));
        let result = match mode.as_deref() {
            Some("steer") => self.state.steer_to_codex(&channel_id, &text).await,
            _ => self.state.send_to_codex(&channel_id, &text).await,
        };
        let content = match result {
            Ok(Some(m)) => format!("✅ {m}"),
            Ok(None) => "⚠️ No thread mapped to this channel.".to_string(),
            Err(e) => format!("❌ {e}"),
        };
        let _ = cmd.create_response(&ctx.http, CreateInteractionResponse::Message(
            CreateInteractionResponseMessage::new().content(content).ephemeral(true),
        )).await;
    }

    async fn cmd_retract(&self, ctx: &Context, cmd: &CommandInteraction) {
        let channel_id = cmd.channel_id.get();
        let result = self.state.retract_pending(&channel_id).await;
        let content = match result {
            Ok(Some(t)) => format!("↩️ Retracted: `{t}`"),
            Ok(None) => "ℹ️ No pending messages.".to_string(),
            Err(e) => format!("❌ {e}"),
        };
        let _ = cmd.create_response(&ctx.http, CreateInteractionResponse::Message(
            CreateInteractionResponseMessage::new().content(content).ephemeral(true),
        )).await;
    }

    async fn cmd_attach(&self, ctx: &Context, cmd: &CommandInteraction) {
        let thread_id = Self::extract_string_option(cmd).unwrap_or_default();
        let channel_id = cmd.channel_id.get();
        let result = self.state.attach_thread(&thread_id, channel_id).await;
        let content = match result {
            Ok(()) => format!("✅ Attached `{thread_id}`."),
            Err(e) => format!("❌ {e}"),
        };
        let _ = cmd.create_response(&ctx.http, CreateInteractionResponse::Message(
            CreateInteractionResponseMessage::new().content(content).ephemeral(true),
        )).await;
    }

    async fn cmd_new(&self, ctx: &Context, cmd: &CommandInteraction) {
        let prompt = Self::extract_string_option(cmd).unwrap_or_default();
        if prompt.is_empty() {
            let _ = cmd.create_response(&ctx.http, CreateInteractionResponse::Message(
                CreateInteractionResponseMessage::new().content("Prompt cannot be empty.").ephemeral(true),
            )).await;
            return;
        }
        // Second option is optional cwd
        let cwd = cmd.data.options.get(1).and_then(|o| {
            if let CommandDataOptionValue::String(s) = &o.value { Some(s.clone()) } else { None }
        }).unwrap_or_else(|| std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).unwrap_or_default());

        let codex = self.state.codex.read().await;
        let codex = match codex.as_ref() {
            Some(c) => c,
            None => {
                let _ = cmd.create_response(&ctx.http, CreateInteractionResponse::Message(
                    CreateInteractionResponseMessage::new().content("Codex not connected.").ephemeral(true),
                )).await;
                return;
            }
        };
        match codex.start_thread(&cwd, "on-request", "read-only", None).await {
            Ok(thread_id) => {
                drop(codex);
                self.state.map_thread(&thread_id, cmd.channel_id.get());
                // Start the first turn in a background task
                let state = self.state.clone();
                let tid = thread_id.clone();
                let p = prompt.clone();
                let ch = cmd.channel_id.get();
                tokio::spawn(async move {
                    let codex = state.codex.read().await;
                    if let Some(codex) = codex.as_ref() {
                        let _ = codex.start_turn(&tid, &p).await;
                    }
                    state.last_turn.insert(ch, tid.clone());
                });
                let content = format!("✅ Started Codex thread `{}` with prompt: {prompt}", &thread_id[..12.min(thread_id.len())]);
                let _ = cmd.create_response(&ctx.http, CreateInteractionResponse::Message(
                    CreateInteractionResponseMessage::new().content(content),
                )).await;
            }
            Err(e) => {
                let _ = cmd.create_response(&ctx.http, CreateInteractionResponse::Message(
                    CreateInteractionResponseMessage::new().content(format!("❌ {e}")).ephemeral(true),
                )).await;
            }
        }
    }
    async fn cmd_threads(&self, ctx: &Context, cmd: &CommandInteraction) {
        let codex = self.state.codex.read().await;
        let result = match codex.as_ref() {
            Some(c) => c.list_threads(10).await,
            None => Err("Codex not connected.".into()),
        };
        drop(codex);
        let content = match result {
            Ok(threads) => {
                if threads.is_empty() {
                    "No Codex threads found.".to_string()
                } else {
                    let mapped = self.state.thread_map.clone();
                    let mut s = String::from("**Recent Codex threads:**\n");
                    for t in threads {
                        let is_mapped = mapped.iter().any(|m| m.key() == &t.id);
                        let marker = if is_mapped { " ✅" } else { "" };
                        let name = t.name.as_deref().unwrap_or(t.preview.as_deref().unwrap_or("(untitled)"));
                        let short = &t.id[..12.min(t.id.len())];
                        s.push_str(&format!("• `{short}`{marker} — {name}\n"));
                    }
                    s.push_str("\nUse `/codex attach <full_thread_id>` to map one to a channel.");
                    s
                }
            }
            Err(e) => format!("❌ {e}"),
        };
        let _ = cmd.create_response(&ctx.http, CreateInteractionResponse::Message(
            CreateInteractionResponseMessage::new().content(content).ephemeral(true),
        )).await;
    }
    async fn cmd_stop(&self, ctx: &Context, cmd: &CommandInteraction) {
        let channel_id = cmd.channel_id.get();
        let thread_id = match self.state.reverse_map.get(&channel_id) {
            Some(v) => v.value().clone(),
            None => {
                let _ = cmd.create_response(&ctx.http, CreateInteractionResponse::Message(
                    CreateInteractionResponseMessage::new().content("No thread mapped to this channel.").ephemeral(true),
                )).await;
                return;
            }
        };
        let turn_id = match self.state.last_turn.get(&channel_id) {
            Some(v) => v.value().clone(),
            None => {
                let _ = cmd.create_response(&ctx.http, CreateInteractionResponse::Message(
                    CreateInteractionResponseMessage::new().content("No active turn.").ephemeral(true),
                )).await;
                return;
            }
        };
        let codex = self.state.codex.read().await;
        let result = match codex.as_ref() {
            Some(c) => c.interrupt_turn(&thread_id, &turn_id).await,
            None => Err("Codex not connected.".into()),
        };
        let content = match result {
            Ok(()) => "🛑 Turn interrupted.".to_string(),
            Err(e) => format!("❌ {e}"),
        };
        let _ = cmd.create_response(&ctx.http, CreateInteractionResponse::Message(
            CreateInteractionResponseMessage::new().content(content).ephemeral(true),
        )).await;
    }
    async fn cmd_model(&self, ctx: &Context, cmd: &CommandInteraction) {
        // Optional "set" option
        let set_to = cmd.data.options.first().and_then(|o| {
            o.value.as_str().map(|s| s.to_string())
        });

        let codex = self.state.codex.read().await;
        let codex = match codex.as_ref() {
            Some(c) => c,
            None => {
                let _ = cmd.create_response(&ctx.http, CreateInteractionResponse::Message(
                    CreateInteractionResponseMessage::new().content("Codex not connected.").ephemeral(true),
                )).await;
                return;
            }
        };

        match codex.list_models().await {
            Ok(models) => {
                drop(codex);
                if let Some(target) = set_to {
                    // Verify model exists
                    let found = models.iter().any(|m| m["id"].as_str() == Some(target.as_str()) || m["model"].as_str() == Some(target.as_str()));
                    if found {
                        if let Ok(mut g) = self.state.default_model.lock() { *g = Some(target.clone()); }
                        let content = format!("✅ New threads will use model `{target}`.");
                        let _ = cmd.create_response(&ctx.http, CreateInteractionResponse::Message(
                            CreateInteractionResponseMessage::new().content(content).ephemeral(true),
                        )).await;
                    } else {
                        let content = format!("❌ Model `{target}` not found. Use `/codex model` to list available models.");
                        let _ = cmd.create_response(&ctx.http, CreateInteractionResponse::Message(
                            CreateInteractionResponseMessage::new().content(content).ephemeral(true),
                        )).await;
                    }
                } else {
                    let mut list = String::from("**Available models:**\n");
                    for m in models.iter().take(20) {
                        let id = m["id"].as_str().unwrap_or("?");
                        let display = m["displayName"].as_str().unwrap_or("");
                        let is_default = m["isDefault"].as_bool().unwrap_or(false);
                        let marker = if is_default { " ⭐" } else { "" };
                        list.push_str(&format!("• `{id}`{marker} — {display}\n"));
                    }
                    let current = self.state.default_model.lock().ok().and_then(|g| g.clone());
                    if let Some(cur) = current {
                        list.push_str(&format!("\n**Current default:** `{cur}`"));
                    }
                    let _ = cmd.create_response(&ctx.http, CreateInteractionResponse::Message(
                        CreateInteractionResponseMessage::new().content(list).ephemeral(true),
                    )).await;
                }
            }
            Err(e) => {
                let _ = cmd.create_response(&ctx.http, CreateInteractionResponse::Message(
                    CreateInteractionResponseMessage::new().content(format!("❌ {e}")).ephemeral(true),
                )).await;
            }
        }
    }
    async fn cmd_detach(&self, ctx: &Context, cmd: &CommandInteraction) {
        let thread_id = Self::extract_string_option(cmd).unwrap_or_default();
        let result = self.state.detach_thread(&thread_id).await;
        let content = match result {
            Ok(()) => format!("✅ Detached `{thread_id}`."),
            Err(e) => format!("❌ {e}"),
        };
        let _ = cmd.create_response(&ctx.http, CreateInteractionResponse::Message(
            CreateInteractionResponseMessage::new().content(content).ephemeral(true),
        )).await;
    }
}

pub async fn post_approval_card(
    http: &Http,
    channel_id: u64,
    title: &str,
    detail: &str,
    token: &str,
) -> Result<MessageId, serenity::Error> {
    let buttons = vec![
        CreateButton::new(format!("approve:{token}")).label("Approve").style(ButtonStyle::Success),
        CreateButton::new(format!("decline:{token}")).label("Reject").style(ButtonStyle::Danger),
    ];
    let embed = CreateEmbed::new().title(title).description(detail).colour(Colour::ORANGE);
    let msg = ChannelId::new(channel_id)
        .send_message(
            http,
            CreateMessage::new().embed(embed).components(vec![
                CreateActionRow::Buttons(buttons),
            ]),
        )
        .await?;
    Ok(msg.id)
}
