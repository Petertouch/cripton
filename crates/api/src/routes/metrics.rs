use axum::Router;
use axum::extract::State;
use axum::http::header;
use axum::response::IntoResponse;
use axum::routing::get;

use crate::server::AppState;

/// Prometheus-compatible metrics endpoint
async fn get_metrics(State(state): State<AppState>) -> impl IntoResponse {
    let rm = state.risk_manager.lock().await;
    let (cb_tripped, window_pnl) = rm.circuit_breaker_status();
    let exposure = rm.current_exposure();
    drop(rm);

    let om = state.order_manager.lock().await;
    let filled = om.filled_count();
    let volume = om.total_volume();
    drop(om);

    let params = state.scheduler.current_params();
    let uptime = (chrono::Utc::now() - state.start_time).num_seconds();

    let paper = if state.paper_mode { 1 } else { 0 };
    let cb = if cb_tripped { 1 } else { 0 };
    let aggressive = if params.is_aggressive { 1 } else { 0 };

    let body = format!(
        r#"# HELP cripton_uptime_seconds Bot uptime in seconds
# TYPE cripton_uptime_seconds gauge
cripton_uptime_seconds {uptime}

# HELP cripton_paper_mode Whether paper trading is active
# TYPE cripton_paper_mode gauge
cripton_paper_mode {paper}

# HELP cripton_circuit_breaker_active Whether circuit breaker is tripped
# TYPE cripton_circuit_breaker_active gauge
cripton_circuit_breaker_active {cb}

# HELP cripton_window_pnl Current window P&L
# TYPE cripton_window_pnl gauge
cripton_window_pnl {window_pnl}

# HELP cripton_current_exposure Current total exposure in USD
# TYPE cripton_current_exposure gauge
cripton_current_exposure {exposure}

# HELP cripton_filled_orders_total Total number of filled orders
# TYPE cripton_filled_orders_total counter
cripton_filled_orders_total {filled}

# HELP cripton_total_volume Total traded volume in USD
# TYPE cripton_total_volume counter
cripton_total_volume {volume}

# HELP cripton_aggressive_mode Whether aggressive trading window is active
# TYPE cripton_aggressive_mode gauge
cripton_aggressive_mode {aggressive}
"#
    );

    ([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], body)
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/metrics", get(get_metrics))
}
