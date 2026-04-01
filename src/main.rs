use std::sync::Arc;

use anyhow::Result;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

use cripton_core::TradingPair;
use cripton_exchanges::BinanceClient;
use cripton_market_data::Collector;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    info!("🚀 Cripton starting...");

    // Load config from environment
    let api_key = std::env::var("BINANCE_API_KEY").unwrap_or_default();
    let api_secret = std::env::var("BINANCE_API_SECRET").unwrap_or_default();

    if api_key.is_empty() || api_secret.is_empty() {
        warn!("BINANCE_API_KEY or BINANCE_API_SECRET not set — running in read-only mode");
    }

    // Initialize exchange connectors
    let binance = Arc::new(BinanceClient::new(api_key, api_secret));

    // Define pairs to monitor
    let pairs = vec![
        TradingPair::UsdtUsdc,
        TradingPair::UsdtEurc,
        TradingPair::EurcUsdc,
    ];

    info!("Monitoring pairs: {:?}", pairs);

    // Start market data collector
    let collector = Collector::new(vec![binance.clone()], pairs);
    let mut state_rx = collector.start().await?;

    info!("Market data collector started. Waiting for updates...");

    // Main loop: receive market state updates and log them
    let mut update_count: u64 = 0;
    while let Some(state) = state_rx.recv().await {
        update_count += 1;

        // Log every 100th update to avoid spam
        if update_count % 100 == 0 {
            info!("--- Market State Update #{} ---", update_count);
            for book in &state.order_books {
                if let (Some(bid), Some(ask)) = (book.best_bid(), book.best_ask()) {
                    let spread_pct = book
                        .spread_pct()
                        .map(|s| format!("{:.4}%", s))
                        .unwrap_or_else(|| "N/A".to_string());

                    info!(
                        "  {} {} | Bid: {} | Ask: {} | Spread: {}",
                        book.exchange, book.pair, bid.price, ask.price, spread_pct
                    );
                }
            }
        }
    }

    warn!("Market data stream ended. Shutting down.");
    Ok(())
}
