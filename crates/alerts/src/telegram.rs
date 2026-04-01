use std::time::Duration;

use anyhow::Result;
use reqwest::Client;
use tracing::{error, info};

const TELEGRAM_API: &str = "https://api.telegram.org";

/// Sends alerts to a Telegram chat via Bot API.
///
/// Setup:
/// 1. Create a bot via @BotFather, get the token
/// 2. Start a chat with the bot or add it to a group
/// 3. Get the chat_id via https://api.telegram.org/bot<TOKEN>/getUpdates
/// 4. Set TELEGRAM_BOT_TOKEN and TELEGRAM_CHAT_ID env vars
#[derive(Clone)]
pub struct TelegramAlerter {
    client: Client,
    bot_token: String,
    chat_id: String,
    enabled: bool,
}

impl TelegramAlerter {
    pub fn from_env() -> Self {
        let bot_token = std::env::var("TELEGRAM_BOT_TOKEN").unwrap_or_default();
        let chat_id = std::env::var("TELEGRAM_CHAT_ID").unwrap_or_default();
        let enabled = !bot_token.is_empty() && !chat_id.is_empty();

        if !enabled {
            info!("Telegram alerts disabled (TELEGRAM_BOT_TOKEN or TELEGRAM_CHAT_ID not set)");
        } else {
            info!("Telegram alerts enabled for chat {}", chat_id);
        }

        Self {
            client: Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .unwrap_or_else(|_| Client::new()),
            bot_token,
            chat_id,
            enabled,
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Send a text message to the configured chat
    pub async fn send(&self, message: &str) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }

        let url = format!("{}/bot{}/sendMessage", TELEGRAM_API, self.bot_token);

        let body = serde_json::json!({
            "chat_id": self.chat_id,
            "text": message,
            "parse_mode": "HTML",
        });

        let resp = self.client.post(&url).json(&body).send().await?;

        if !resp.status().is_success() {
            error!("Telegram alert failed: HTTP {}", resp.status());
        }

        Ok(())
    }

    // --- Convenience methods for common alerts ---

    pub async fn alert_circuit_breaker(&self, window_pnl: &str) {
        let msg = format!(
            "<b>CIRCUIT BREAKER TRIPPED</b>\nWindow P&amp;L: {}\nTrading halted until cooldown expires.",
            window_pnl
        );
        if let Err(e) = self.send(&msg).await {
            error!("Failed to send circuit breaker alert: {}", e);
        }
    }

    pub async fn alert_trade_executed(&self, legs: usize, total: u64) {
        let msg = format!("Trade executed: {} legs | Total trades: {}", legs, total);
        if let Err(e) = self.send(&msg).await {
            error!("Failed to send trade alert: {}", e);
        }
    }

    pub async fn alert_partial_fill(&self, filled: usize, expected: usize) {
        let msg = format!(
            "<b>PARTIAL FILL WARNING</b>\n{}/{} legs executed.\nManual review required — possible unhedged position.",
            filled, expected
        );
        if let Err(e) = self.send(&msg).await {
            error!("Failed to send partial fill alert: {}", e);
        }
    }

    pub async fn alert_error(&self, context: &str) {
        let msg = format!("<b>ERROR</b>\n{}", context);
        if let Err(e) = self.send(&msg).await {
            error!("Failed to send error alert: {}", e);
        }
    }

    pub async fn alert_startup(&self, mode: &str, exchanges: usize, strategies: usize) {
        let msg = format!(
            "Cripton started\nMode: {}\nExchanges: {}\nStrategies: {}",
            mode, exchanges, strategies
        );
        if let Err(e) = self.send(&msg).await {
            error!("Failed to send startup alert: {}", e);
        }
    }
}
