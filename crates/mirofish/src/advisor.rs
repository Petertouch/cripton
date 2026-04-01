use std::time::Duration;

use anyhow::{Context, Result};
use reqwest::Client;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

/// Capital allocation recommendation from MiroFish swarm
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AllocationAdvice {
    /// Recommended capital allocation per strategy (0.0 - 1.0)
    pub triangular_weight: Decimal,
    pub cross_exchange_weight: Decimal,
    /// Recommended aggression multiplier (0.5 - 3.0)
    pub aggression: Decimal,
    /// Confidence of the swarm consensus (0.0 - 1.0)
    pub confidence: Decimal,
    /// Human-readable reasoning
    pub reasoning: String,
    /// Timestamp of the recommendation
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

impl Default for AllocationAdvice {
    fn default() -> Self {
        Self {
            triangular_weight: dec!(0.5),
            cross_exchange_weight: dec!(0.5),
            aggression: dec!(1.0),
            confidence: dec!(0.5),
            reasoning: "default allocation (MiroFish not available)".to_string(),
            timestamp: chrono::Utc::now(),
        }
    }
}

/// Client that queries MiroFish swarm intelligence for strategic decisions.
///
/// MiroFish runs multiple AI agents with different trading profiles,
/// each "votes" on capital allocation. The consensus becomes the recommendation.
///
/// Queries are slow (30-60s) — run once per hour, not per trade cycle.
pub struct MiroFishAdvisor {
    client: Client,
    base_url: String,
    enabled: bool,
}

impl MiroFishAdvisor {
    pub fn new(base_url: &str) -> Self {
        let enabled = !base_url.is_empty();
        if enabled {
            info!("MiroFish advisor connected: {}", base_url);
        } else {
            info!("MiroFish advisor disabled (no URL configured)");
        }

        Self {
            client: Client::builder()
                .timeout(Duration::from_secs(90)) // swarm deliberation is slow
                .build()
                .unwrap_or_else(|_| Client::new()),
            base_url: base_url.to_string(),
            enabled,
        }
    }

    pub fn from_env() -> Self {
        let url = std::env::var("MIROFISH_URL").unwrap_or_default();
        Self::new(&url)
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Query the swarm for capital allocation advice.
    /// Returns default allocation if MiroFish is unavailable.
    pub async fn get_allocation(&self, context: &AllocationContext) -> AllocationAdvice {
        if !self.enabled {
            return AllocationAdvice::default();
        }

        match self.query_swarm(context).await {
            Ok(advice) => {
                info!(
                    "MiroFish advice: tri={:.0}% cross={:.0}% aggr={:.1}x conf={:.0}% | {}",
                    advice.triangular_weight * dec!(100),
                    advice.cross_exchange_weight * dec!(100),
                    advice.aggression,
                    advice.confidence * dec!(100),
                    advice.reasoning
                );
                advice
            }
            Err(e) => {
                warn!("MiroFish query failed: {} — using defaults", e);
                AllocationAdvice::default()
            }
        }
    }

    async fn query_swarm(&self, context: &AllocationContext) -> Result<AllocationAdvice> {
        let prompt = format!(
            "You are a portfolio allocation advisor for a stablecoin arbitrage bot. \
             Current market state: triangular spread={:.4}%, cross-exchange COP spread={:.4}%. \
             Recent P&L: {}. Circuit breaker: {}. \
             Recommend capital allocation weights (0-1) for triangular and cross-exchange strategies, \
             plus an aggression multiplier (0.5-3.0). Respond in JSON.",
            context.triangular_spread_pct,
            context.cross_exchange_spread_pct,
            context.recent_pnl,
            if context.circuit_breaker_active {
                "ACTIVE"
            } else {
                "inactive"
            }
        );

        let body = serde_json::json!({
            "prompt": prompt,
            "simulation_id": "cripton_allocation",
        });

        let resp = self
            .client
            .post(&format!("{}/api/simulate", self.base_url))
            .json(&body)
            .send()
            .await
            .context("MiroFish request failed")?;

        if !resp.status().is_success() {
            anyhow::bail!("MiroFish returned HTTP {}", resp.status());
        }

        let result: serde_json::Value = resp.json().await?;

        // Parse the swarm's recommendation
        let advice = AllocationAdvice {
            triangular_weight: result
                .get("triangular_weight")
                .and_then(|v| v.as_f64())
                .map(|f| Decimal::try_from(f).unwrap_or(dec!(0.5)))
                .unwrap_or(dec!(0.5))
                .max(Decimal::ZERO)
                .min(Decimal::ONE),
            cross_exchange_weight: result
                .get("cross_exchange_weight")
                .and_then(|v| v.as_f64())
                .map(|f| Decimal::try_from(f).unwrap_or(dec!(0.5)))
                .unwrap_or(dec!(0.5))
                .max(Decimal::ZERO)
                .min(Decimal::ONE),
            aggression: result
                .get("aggression")
                .and_then(|v| v.as_f64())
                .map(|f| Decimal::try_from(f).unwrap_or(dec!(1.0)))
                .unwrap_or(dec!(1.0))
                .max(dec!(0.5))
                .min(dec!(3.0)),
            confidence: result
                .get("confidence")
                .and_then(|v| v.as_f64())
                .map(|f| Decimal::try_from(f).unwrap_or(dec!(0.5)))
                .unwrap_or(dec!(0.5))
                .max(Decimal::ZERO)
                .min(Decimal::ONE),
            reasoning: result
                .get("reasoning")
                .and_then(|v| v.as_str())
                .unwrap_or("no reasoning provided")
                .to_string(),
            timestamp: chrono::Utc::now(),
        };

        Ok(advice)
    }
}

/// Context provided to MiroFish for decision-making
#[derive(Debug, Serialize)]
pub struct AllocationContext {
    pub triangular_spread_pct: Decimal,
    pub cross_exchange_spread_pct: Decimal,
    pub recent_pnl: String,
    pub circuit_breaker_active: bool,
}
