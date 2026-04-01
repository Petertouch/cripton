use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;

use crate::server::AppState;

#[derive(Serialize)]
struct TradesSummary {
    total_trades: Option<i64>,
    today_trades: Option<i64>,
    today_volume: Option<String>,
    today_fees: Option<String>,
    db_connected: bool,
}

async fn get_trades(State(state): State<AppState>) -> Json<TradesSummary> {
    if let Some(ref store) = state.storage {
        let total = store.total_trades().await.ok();
        let today = store.today_summary().await.ok();

        Json(TradesSummary {
            total_trades: total,
            today_trades: today.as_ref().map(|t| t.0),
            today_volume: today.as_ref().map(|t| format!("{:.2}", t.1)),
            today_fees: today.as_ref().map(|t| format!("{:.6}", t.2)),
            db_connected: true,
        })
    } else {
        Json(TradesSummary {
            total_trades: None,
            today_trades: None,
            today_volume: None,
            today_fees: None,
            db_connected: false,
        })
    }
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/trades", get(get_trades))
}
