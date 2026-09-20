use std::env;

#[derive(Debug, Clone)]
pub struct Config {
    pub discord_bot_token: String,
    pub discord_application_id: u64,
    pub discord_guild_id: u64,
    pub controller_user_id: u64,
    pub codex_command: String,
    pub codex_listen_url: String,
    pub codex_port: u16,
}

impl Config {
    pub fn from_env() -> Result<Self, String> {
        let _ = dotenvy::dotenv();

        let bot_token = env::var("DISCORD_BOT_TOKEN")
            .map_err(|_| "DISCORD_BOT_TOKEN is required in .env")?;
        let app_id: u64 = env::var("DISCORD_APPLICATION_ID")
            .map_err(|_| "DISCORD_APPLICATION_ID is required")?
            .parse()
            .map_err(|_| "DISCORD_APPLICATION_ID must be a number")?;
        let guild_id: u64 = env::var("DISCORD_GUILD_ID")
            .map_err(|_| "DISCORD_GUILD_ID is required")?
            .parse()
            .map_err(|_| "DISCORD_GUILD_ID must be a number")?;
        let controller: u64 = env::var("DISCORD_CONTROLLER_USER_ID")
            .map_err(|_| "DISCORD_CONTROLLER_USER_ID is required")?
            .parse()
            .map_err(|_| "DISCORD_CONTROLLER_USER_ID must be a number")?;
        let codex_cmd = env::var("CODEX_COMMAND").unwrap_or_else(|_| "codex".to_string());
        let listen_url =
            env::var("CODEX_APP_SERVER_LISTEN_URL").unwrap_or_else(|_| "ws://127.0.0.1:8837".into());
        let port: u16 = listen_url
            .rsplit(':')
            .next()
            .and_then(|p| p.parse().ok())
            .unwrap_or(8837);

        Ok(Self {
            discord_bot_token: bot_token,
            discord_application_id: app_id,
            discord_guild_id: guild_id,
            controller_user_id: controller,
            codex_command: codex_cmd,
            codex_listen_url: listen_url,
            codex_port: port,
        })
    }
}