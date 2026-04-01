use std::sync::Arc;

use anyhow::Result;
use rust_decimal_macros::dec;
use tokio::sync::Mutex;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

use cripton_core::{Exchange, TradingPair};
use cripton_exchanges::BinanceClient;
use cripton_execution::{ExecutionConfig, ExecutionEngine};
use cripton_market_data::Collector;
use cripton_risk::{RiskConfig, RiskManager};
use cripton_storage::PgStorage;
use cripton_strategy::{Strategy, TriangularArbitrage};

#[tokio::main]
async fn main() -> Result<()> {
    // SEC: default to "warn" in production to avoid leaking strategy details
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn")),
        )
        .init();

    info!("=== CRIPTON Stablecoin Arbitrage Bot ===");

    // --- Config from env ---
    // SEC: credentials are moved into BinanceClient and zeroized from this scope
    let api_key = std::env::var("BINANCE_API_KEY").unwrap_or_default();
    let api_secret = std::env::var("BINANCE_API_SECRET").unwrap_or_default();
    let database_url = std::env::var("DATABASE_URL").ok();

    let read_only = api_key.is_empty() || api_secret.is_empty();
    if read_only {
        warn!("API credentials not configured — running in READ-ONLY mode");
    }

    // --- Storage (optional) ---
    // SEC: don't log the database URL or connection errors that may contain credentials
    let storage = if let Some(ref url) = database_url {
        match PgStorage::new(url).await {
            Ok(s) => {
                info!("Database connected");
                Some(Arc::new(s))
            }
            Err(_) => {
                warn!("Database not available, running without persistence");
                None
            }
        }
    } else {
        warn!("Database not configured, running without persistence");
        None
    };

    // --- Exchange connectors ---
    // SEC: BinanceClient takes ownership and zeroizes original strings
    let binance = Arc::new(BinanceClient::new(api_key, api_secret));

    // --- Pairs to monitor ---
    let pairs = vec![
        TradingPair::UsdtUsdc,
        TradingPair::UsdtEurc,
        TradingPair::EurcUsdc,
    ];

    info!("Monitoring {} pairs on Binance", pairs.len());

    // --- Strategy ---
    let strategy = TriangularArbitrage::new(
        dec!(0.03),   // min 0.03% profit to trigger
        dec!(0.001),  // 0.1% fee per trade
        dec!(100),    // trade $100 per cycle
        Exchange::Binance,
    );

    // --- Risk Manager ---
    let risk_config = RiskConfig {
        max_trade_amount: dec!(500),
        max_total_exposure: dec!(2000),
        max_loss: dec!(50),
        cb_window_minutes: 60,
        max_consecutive_losses: 5,
        cb_cooldown_minutes: 30,
    };
    let risk_manager = Arc::new(Mutex::new(RiskManager::new(risk_config)));

    // --- Execution Engine ---
    let exec_config = ExecutionConfig {
        max_slippage_pct: dec!(0.05),
        max_retries: 2,
        use_limit_orders: true,
    };
    let execution = ExecutionEngine::new(vec![binance.clone()], exec_config);

    // --- Market Data Collector ---
    let collector = Collector::new(vec![binance.clone()], pairs);
    let mut state_rx = collector.start().await?;

    info!("All systems online. Entering main trading loop...");
    if read_only {
        info!("  Mode: READ-ONLY (monitoring only)");
    } else {
        info!("  Mode: LIVE TRADING");
    }

    // --- Main Loop ---
    let mut update_count: u64 = 0;
    let mut opportunities_found: u64 = 0;
    let mut trades_executed: u64 = 0;

    while let Some(state) = state_rx.recv().await {
        update_count = update_count.saturating_add(1);

        // Log market status periodically
        if update_count % 500 == 0 {
            info!("--- Status: {} updates | {} opportunities | {} trades ---",
                update_count, opportunities_found, trades_executed);

            // Log circuit breaker status
            let rm = risk_manager.lock().await;
            let (tripped, _) = rm.circuit_breaker_status();
            if tripped {
                warn!("  Circuit breaker: ACTIVE");
            }
        }

        // --- 1. Evaluate strategy ---
        let signals = strategy.evaluate(&state).await;

        if signals.is_empty() {
            continue;
        }

        opportunities_found = opportunities_found.saturating_add(1);

        // --- 2. Validate through risk manager ---
        let approved_signals = {
            let mut rm = risk_manager.lock().await;
            rm.validate(&signals)
        };

        if approved_signals.is_empty() {
            continue;
        }

        // --- 3. Execute (only if not read-only) ---
        if read_only {
            info!("READ-ONLY: Would execute {} signals", approved_signals.len());
            continue;
        }

        match execution.execute_signals(&approved_signals, &state).await {
            Ok(trades) => {
                trades_executed = trades_executed.saturating_add(trades.len() as u64);

                // SEC: track trade value for exposure release
                let trade_exposure: rust_decimal::Decimal = trades
                    .iter()
                    .map(|t| t.quantity * t.price)
                    .sum();
                let total_fees: rust_decimal::Decimal = trades.iter().map(|t| t.fee).sum();
                let pnl = -total_fees;

                // Record trades in storage — retry once on failure
                if let Some(ref store) = storage {
                    if let Err(_) = store.insert_trades(&trades).await {
                        warn!("Trade persistence failed, retrying...");
                        if let Err(_) = store.insert_trades(&trades).await {
                            // SEC: if persistence fails twice, halt trading to prevent
                            // unrecorded trades accumulating
                            error!("Trade persistence failed twice — halting to prevent unrecorded trades");
                            break;
                        }
                    }
                }

                // SEC: record P&L and release exposure atomically
                let mut rm = risk_manager.lock().await;
                rm.record_trade_result(pnl, trade_exposure);

                info!("Executed {} trades | total: {}", trades.len(), trades_executed);
            }
            Err(_) => {
                error!("Execution failed");
            }
        }
    }

    warn!("Market data stream ended. Shutting down.");
    Ok(())
}
