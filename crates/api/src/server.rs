use std::sync::Arc;

use anyhow::Result;
use axum::Router;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::Response;
use tokio::sync::Mutex;
use tower_http::cors::{AllowOrigin, CorsLayer};
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
    pub api_token: String,
}

/// SEC: Bearer token authentication middleware
async fn auth_middleware(
    state: axum::extract::State<AppState>,
    request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    // Health check is public
    if request.uri().path() == "/api/health" {
        return Ok(next.run(request).await);
    }

    // If no token configured, allow all (dev mode)
    if state.api_token.is_empty() {
        return Ok(next.run(request).await);
    }

    let auth_header = request
        .headers()
        .get("Authorization")
        .and_then(|v| v.to_str().ok());

    match auth_header {
        Some(header) if header.starts_with("Bearer ") => {
            let token = &header[7..];
            if token == state.api_token {
                Ok(next.run(request).await)
            } else {
                Err(StatusCode::UNAUTHORIZED)
            }
        }
        _ => Err(StatusCode::UNAUTHORIZED),
    }
}

/// Start the API server on the given port
pub async fn start_api(state: AppState, port: u16) -> Result<()> {
    // SEC: CORS restricted to localhost origins only
    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::list([
            "http://localhost:3000".parse()?,
            "http://127.0.0.1:3000".parse()?,
        ]))
        .allow_methods(tower_http::cors::Any)
        .allow_headers(tower_http::cors::Any);

    let app = Router::new()
        .nest("/api", routes::router())
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ))
        .layer(cors)
        .with_state(state);

    // SEC: bind to localhost only — not accessible from external network
    let addr = format!("127.0.0.1:{}", port);
    info!("API server starting on {}", addr);

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
