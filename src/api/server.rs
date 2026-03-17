//! HTTP server setup and configuration.
//!
//! This module configures the Axum HTTP server with routing, CORS, and error handling.

use axum::{
    Router,
    routing::{get, post},
};
use std::net::SocketAddr;
use tower::ServiceBuilder;
use tower_http::{
    cors::{Any, CorsLayer},
    trace::TraceLayer,
};

use crate::api::handlers;

/// Start the HTTP server
pub async fn start_server(
    addr: SocketAddr,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let app = create_router();

    let listener = tokio::net::TcpListener::bind(addr).await?;
    #[cfg(feature = "cli")]
    {
        tracing::info!("Server listening on {}", addr);
    }
    #[cfg(not(feature = "cli"))]
    {
        eprintln!("Server listening on {}", addr);
    }

    axum::serve(listener, app).await?;

    Ok(())
}

/// Create the API router
fn create_router() -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    Router::new()
        .route("/api/v1/health", get(handlers::health_check))
        .route(
            "/api/v1/context/summarize",
            post(handlers::context_summarize_handler),
        )
        .route(
            "/api/v1/context/synthesize",
            post(handlers::context_synthesize_handler),
        )
        .route(
            "/api/v1/context/graphs",
            post(handlers::context_graphs_handler),
        )
        .layer(
            ServiceBuilder::new()
                .layer(TraceLayer::new_for_http())
                .layer(cors),
        )
}
