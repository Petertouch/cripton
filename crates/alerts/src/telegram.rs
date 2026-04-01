use std::time::Duration;

use anyhow::Result;
use reqwest::Client;
use secrecy::{ExposeSecret, SecretString};
use tracing::{error, info};

const TELEGRAM_API: &str = "https://api.telegram.org";

/// SEC: sanitize text to prevent HTML injection in Telegram messages
fn sanitize_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

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
    bot_token: SecretString,
    chat_id: String,
    enabled: bool,
}

impl TelegramAlerter {
    pub fn from_env() -> Self {
        let bot_token = std::env::var("TELEGRAM_BOT_TOKEN").unwrap_or_default();
        let chat_id = std::env::var("TELEGRAM_CHAT_ID").unwrap_or_default();
        let enabled = !bot_token.is_empty() && !chat_id.is_empty();

        if !enabled {
            info!("Telegram alerts disabled (credentials not set)");
        } else {
            info!("Telegram alerts enabled");
        }

        Self {
            client: Client::builder()
                .timeout(Duration::from_secs(5)) // SEC: reduced from 10s to 5s
                .build()
                .unwrap_or_else(|_| Client::new()),
            bot_token: SecretString::from(bot_token),
            chat_id,
            enabled,
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Send a text message to the configured chat.
    /// SEC: spawns as background task to never block the trading loop.
    pub fn send_async(&self, message: String) {
        if !self.enabled {
            return;
        }
        let client = self.client.clone();
        let url = format!(
            "{}/bot{}/sendMessage",
            TELEGRAM_API,
            self.bot_token.expose_secret()
        );
        let chat_id = self.chat_id.clone();

        tokio::spawn(async move {
            let body = serde_json::json!({
                "chat_id": chat_id,
                "text": message,
                "parse_mode": "HTML",
            });

            match client.post(&url).json(&body).send().await {
                Ok(resp) if !resp.status().is_success() => {
                    error!("Telegram alert failed: HTTP {}", resp.status());
                }
                Err(e) => {
                    error!("Telegram alert error: {}", e);
                }
                _ => {}
            }
        });
    }

    /// Blocking send for critical alerts that MUST be delivered
    pub async fn send_critical(&self, message: &str) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }

        let url = format!(
            "{}/bot{}/sendMessage",
            TELEGRAM_API,
            self.bot_token.expose_secret()
        );

        let body = serde_json::json!({
            "chat_id": self.chat_id,
            "text": message,
            "parse_mode": "HTML",
        });

        let resp = self.client.post(&url).json(&body).send().await?;
        if !resp.status().is_success() {
            error!("Telegram critical alert failed: HTTP {}", resp.status());
        }
        Ok(())
    }

    // --- Convenience methods ---

    pub fn alert_circuit_breaker(&self, window_pnl: &str) {
        let safe_pnl = sanitize_html(window_pnl);
        self.send_async(format!(
            "<b>CIRCUIT BREAKER TRIPPED</b>\nWindow P&amp;L: {}\nTrading halted.",
            safe_pnl
        ));
    }

    pub fn alert_trade_executed(&self, legs: usize, total: u64) {
        self.send_async(format!(
            "Trade executed: {} legs | Total trades: {}",
            legs, total
        ));
    }

    pub fn alert_partial_fill(&self, filled: usize, expected: usize) {
        self.send_async(format!(
            "<b>PARTIAL FILL WARNING</b>\n{}/{} legs executed.\nManual review required.",
            filled, expected
        ));
    }

    pub fn alert_error(&self, context: &str) {
        let safe_ctx = sanitize_html(context);
        self.send_async(format!("<b>ERROR</b>\n{}", safe_ctx));
    }

    pub fn alert_startup(&self, mode: &str, exchanges: usize, strategies: usize) {
        let safe_mode = sanitize_html(mode);
        self.send_async(format!(
            "Cripton started\nMode: {}\nExchanges: {}\nStrategies: {}",
            safe_mode, exchanges, strategies
        ));
    }
}
