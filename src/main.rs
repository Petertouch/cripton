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
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    info!("=== CRIPTON Stablecoin Arbitrage Bot ===");

    // --- Config from env ---
    let api_key = std::env::var("BINANCE_API_KEY").unwrap_or_default();
    let api_secret = std::env::var("BINANCE_API_SECRET").unwrap_or_default();
    let database_url = std::env::var("DATABASE_URL").ok();

    let read_only = api_key.is_empty() || api_secret.is_empty();
    if read_only {
        warn!("API keys not set — running in READ-ONLY mode (no orders will be placed)");
    }

    // --- Storage (optional) ---
    let storage = if let Some(ref url) = database_url {
        match PgStorage::new(url).await {
            Ok(s) => {
                info!("PostgreSQL connected");
                Some(Arc::new(s))
            }
            Err(e) => {
                warn!("PostgreSQL not available ({}), running without persistence", e);
                None
            }
        }
    } else {
        warn!("DATABASE_URL not set, running without persistence");
        None
    };

    // --- Exchange connectors ---
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
    info!("  Strategy: {} (min profit: 0.03%)", strategy.name());
    info!("  Risk: max $500/trade, $2000 exposure, $50 circuit breaker");
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
        update_count += 1;

        // Log market status periodically
        if update_count % 500 == 0 {
            info!("--- Status: {} updates | {} opportunities | {} trades ---",
                update_count, opportunities_found, trades_executed);
            for book in &state.order_books {
                if let Some(spread_pct) = book.spread_pct() {
                    info!(
                        "  {} {} | spread: {:.4}%",
                        book.exchange, book.pair, spread_pct
                    );
                }
            }

            // Log circuit breaker status
            let rm = risk_manager.lock().await;
            let (tripped, window_pnl) = rm.circuit_breaker_status();
            if tripped {
                warn!("  Circuit breaker: TRIPPED (window P&L: {:.4})", window_pnl);
            }
        }

        // --- 1. Evaluate strategy ---
        let signals = strategy.evaluate(&state).await;

        if signals.is_empty() {
            continue;
        }

        opportunities_found += 1;

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
            info!(
                "READ-ONLY: Would execute {} signals ({})",
                approved_signals.len(),
                approved_signals.first().map(|s| s.reason.as_str()).unwrap_or("")
            );
            continue;
        }

        match execution.execute_signals(&approved_signals, &state).await {
            Ok(trades) => {
                trades_executed += trades.len() as u64;

                // Record trades in storage
                if let Some(ref store) = storage {
                    if let Err(e) = store.insert_trades(&trades).await {
                        error!("Failed to persist trades: {}", e);
                    }
                }

                // Calculate P&L for risk manager
                // For triangular arb, P&L = final amount - initial amount
                let total_fees: rust_decimal::Decimal = trades.iter().map(|t| t.fee).sum();
                let pnl = -total_fees; // Simplified — real P&L comes from price differences

                let mut rm = risk_manager.lock().await;
                rm.record_trade_pnl(pnl);

                info!(
                    "Executed {} trades | fees: {:.6} | total trades: {}",
                    trades.len(),
                    total_fees,
                    trades_executed
                );
            }
            Err(e) => {
                error!("Execution failed: {}", e);
            }
        }
    }

    warn!("Market data stream ended. Shutting down.");
    Ok(())
}
