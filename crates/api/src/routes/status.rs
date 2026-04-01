use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;

use crate::server::AppState;

#[derive(Serialize)]
struct StatusResponse {
    status: String,
    uptime_seconds: i64,
    paper_mode: bool,
    circuit_breaker_active: bool,
    window_pnl: String,
    current_exposure: String,
    active_window: Option<String>,
    is_aggressive: bool,
    filled_orders: usize,
    total_volume: String,
}

async fn get_status(State(state): State<AppState>) -> Json<StatusResponse> {
    let rm = state.risk_manager.lock().await;
    let (cb_tripped, window_pnl) = rm.circuit_breaker_status();
    let exposure = rm.current_exposure();
    drop(rm);

    let om = state.order_manager.lock().await;
    let filled = om.filled_count();
    let volume = om.total_volume();
    drop(om);

    let params = state.scheduler.current_params();
    let uptime = chrono::Utc::now() - state.start_time;

    Json(StatusResponse {
        status: if cb_tripped {
            "circuit_breaker_active".to_string()
        } else {
            "running".to_string()
        },
        uptime_seconds: uptime.num_seconds(),
        paper_mode: state.paper_mode,
        circuit_breaker_active: cb_tripped,
        window_pnl: format!("{:.6}", window_pnl),
        current_exposure: format!("{:.2}", exposure),
        active_window: params.active_window,
        is_aggressive: params.is_aggressive,
        filled_orders: filled,
        total_volume: format!("{:.2}", volume),
    })
}

#[derive(Serialize)]
struct HealthResponse {
    ok: bool,
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse { ok: true })
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/status", get(get_status))
        .route("/health", get(health))
}
