use std::sync::Arc;

use anyhow::Result;
use rust_decimal_macros::dec;
use tokio::sync::Mutex;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

use cripton_core::{Exchange, TradingPair};
use cripton_exchanges::{BinanceClient, BitsoClient};
use cripton_execution::{ExecutionConfig, ExecutionEngine};
use cripton_market_data::Collector;
use cripton_risk::{RiskConfig, RiskManager};
use cripton_scheduler::{Scheduler, SchedulerConfig};
use cripton_storage::PgStorage;
use cripton_strategy::{CrossExchangeArbitrage, CrossPairConfig, Strategy, TriangularArbitrage};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn")),
        )
        .init();

    info!("=== CRIPTON Stablecoin Arbitrage Bot ===");

    // --- Config from env ---
    let binance_key = std::env::var("BINANCE_API_KEY").unwrap_or_default();
    let binance_secret = std::env::var("BINANCE_API_SECRET").unwrap_or_default();
    let bitso_key = std::env::var("BITSO_API_KEY").unwrap_or_default();
    let bitso_secret = std::env::var("BITSO_API_SECRET").unwrap_or_default();
    let database_url = std::env::var("DATABASE_URL").ok();

    let binance_active = !binance_key.is_empty() && !binance_secret.is_empty();
    let bitso_active = !bitso_key.is_empty() && !bitso_secret.is_empty();
    let read_only = !binance_active && !bitso_active;

    if !binance_active {
        warn!("Binance credentials not set");
    }
    if !bitso_active {
        warn!("Bitso credentials not set");
    }
    if read_only {
        warn!("No exchange credentials — running in READ-ONLY mode");
    }

    // --- Storage (optional) ---
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
    let binance: Arc<dyn cripton_exchanges::ExchangeConnector> =
        Arc::new(BinanceClient::new(binance_key, binance_secret));
    let bitso: Arc<dyn cripton_exchanges::ExchangeConnector> =
        Arc::new(BitsoClient::new(bitso_key, bitso_secret));

    let all_exchanges: Vec<Arc<dyn cripton_exchanges::ExchangeConnector>> =
        vec![binance.clone(), bitso.clone()];

    // --- Pairs to monitor ---
    let binance_pairs = vec![
        TradingPair::UsdtUsdc,
        TradingPair::UsdtEurc,
        TradingPair::EurcUsdc,
    ];
    let bitso_pairs = vec![TradingPair::UsdtCop];
    let all_pairs: Vec<TradingPair> = binance_pairs
        .iter()
        .chain(bitso_pairs.iter())
        .copied()
        .collect();

    info!(
        "Monitoring {} pairs across {} exchanges",
        all_pairs.len(),
        all_exchanges.len()
    );

    // --- Scheduler ---
    let scheduler = Scheduler::new(SchedulerConfig {
        base_trade_amount: dec!(100),
        base_min_profit_pct: dec!(0.03),
        allow_off_window: true,
    });

    // --- Strategies ---
    // Strategy 1: Triangular arbitrage on Binance stablecoins
    let triangular =
        TriangularArbitrage::new(dec!(0.03), dec!(0.001), dec!(100), Exchange::Binance);

    // Strategy 2: Cross-exchange COP arbitrage (Binance ↔ Bitso)
    let cross_exchange = CrossExchangeArbitrage::new(
        dec!(0.1),   // 0.1% min profit (COP spread is usually 0.5-1.5%)
        dec!(0.001), // Binance 0.1%
        dec!(0.006), // Bitso 0.6%
        dec!(100),
        vec![CrossPairConfig {
            pair: TradingPair::UsdtCop,
            exchange_a: Exchange::Binance,
            exchange_b: Exchange::Bitso,
        }],
    );

    let strategies: Vec<Box<dyn Strategy>> = vec![Box::new(triangular), Box::new(cross_exchange)];

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
    let execution = ExecutionEngine::new(all_exchanges.clone(), exec_config);

    // --- Market Data Collector ---
    let collector = Collector::new(all_exchanges, all_pairs);
    let mut state_rx = collector.start().await?;

    // --- Bitso polling (no WebSocket yet — poll every 2 seconds) ---
    let bitso_poll = bitso.clone();
    let bitso_poll_pairs = bitso_pairs.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(2));
        loop {
            interval.tick().await;
            for pair in &bitso_poll_pairs {
                if let Ok(book) = bitso_poll.fetch_orderbook(*pair).await {
                    if book.is_valid() {
                        // Book is fetched and validated — collector handles caching
                        // via the initial REST snapshot mechanism
                    }
                }
            }
        }
    });

    info!("All systems online. Entering main trading loop...");
    info!(
        "  Strategies: {} active ({} triangular + {} cross-exchange)",
        strategies.len(),
        1,
        1
    );
    if read_only {
        info!("  Mode: READ-ONLY");
    } else {
        info!("  Mode: LIVE TRADING");
    }

    // --- Main Loop ---
    let mut update_count: u64 = 0;
    let mut opportunities_found: u64 = 0;
    let mut trades_executed: u64 = 0;

    while let Some(state) = state_rx.recv().await {
        update_count = update_count.saturating_add(1);

        // Get dynamic params from scheduler
        let params = scheduler.current_params();

        // Skip if scheduler says zero trade amount (outside windows)
        if params.trade_amount.is_zero() {
            continue;
        }

        // Log status periodically
        if update_count.is_multiple_of(500) {
            let window_name = params.active_window.as_deref().unwrap_or("none");
            info!(
                "--- Status: {} updates | {} opps | {} trades | window: {} | aggressive: {} ---",
                update_count,
                opportunities_found,
                trades_executed,
                window_name,
                params.is_aggressive
            );

            let rm = risk_manager.lock().await;
            let (tripped, _) = rm.circuit_breaker_status();
            if tripped {
                warn!("  Circuit breaker: ACTIVE");
            }
        }

        // --- Evaluate all strategies ---
        let mut all_signals = Vec::new();
        for strategy in &strategies {
            let signals = strategy.evaluate(&state).await;
            all_signals.extend(signals);
        }

        if all_signals.is_empty() {
            continue;
        }

        opportunities_found = opportunities_found.saturating_add(1);

        // --- Apply scheduler params (override trade amount + min profit) ---
        for signal in &mut all_signals {
            if signal.quantity == dec!(100) {
                // Override base amount with scheduler's dynamic amount
                signal.quantity = params.trade_amount;
            }
        }

        // --- Validate through risk manager ---
        let approved_signals = {
            let mut rm = risk_manager.lock().await;
            rm.validate(&all_signals)
        };

        if approved_signals.is_empty() {
            continue;
        }

        // --- Execute ---
        if read_only {
            info!(
                "READ-ONLY: Would execute {} signals",
                approved_signals.len()
            );
            continue;
        }

        match execution.execute_signals(&approved_signals, &state).await {
            Ok(trades) => {
                trades_executed = trades_executed.saturating_add(trades.len() as u64);

                let trade_exposure: rust_decimal::Decimal =
                    trades.iter().map(|t| t.quantity * t.price).sum();
                let total_fees: rust_decimal::Decimal = trades.iter().map(|t| t.fee).sum();
                let pnl = -total_fees;

                if let Some(ref store) = storage
                    && store.insert_trades(&trades).await.is_err()
                {
                    warn!("Trade persistence failed, retrying...");
                    if store.insert_trades(&trades).await.is_err() {
                        error!("Trade persistence failed twice — halting");
                        break;
                    }
                }

                let mut rm = risk_manager.lock().await;
                rm.record_trade_result(pnl, trade_exposure);

                info!(
                    "Executed {} trades | total: {}",
                    trades.len(),
                    trades_executed
                );
            }
            Err(_) => {
                error!("Execution failed");
            }
        }
    }

    warn!("Market data stream ended. Shutting down.");
    Ok(())
}
