use std::sync::Arc;

use anyhow::Result;
use axum::Router;
use tokio::sync::Mutex;
use tower_http::cors::{Any, CorsLayer};
use tracing::info;

use cripton_execution::OrderManager;
use cripton_risk::RiskManager;
use cripton_scheduler::Scheduler;
use cripton_storage::PgStorage;

use crate::routes;

/// Shared state available to all API handlers
#[derive(Clone)]
pub struct AppState {
    pub risk_manager: Arc<Mutex<RiskManager>>,
    pub order_manager: Arc<Mutex<OrderManager>>,
    pub scheduler: Arc<Scheduler>,
    pub storage: Option<Arc<PgStorage>>,
    pub paper_mode: bool,
    pub start_time: chrono::DateTime<chrono::Utc>,
}

/// Start the API server on the given port
pub async fn start_api(state: AppState, port: u16) -> Result<()> {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = Router::new()
        .nest("/api", routes::router())
        .layer(cors)
        .with_state(state);

    let addr = format!("0.0.0.0:{}", port);
    info!("API server starting on {}", addr);

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
