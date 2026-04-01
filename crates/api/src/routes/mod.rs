mod metrics;
mod status;
mod trades;

use axum::Router;

use crate::server::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .merge(status::routes())
        .merge(trades::routes())
        .merge(metrics::routes())
}
