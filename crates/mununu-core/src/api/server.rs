//! HTTP server setup and configuration.
//!
//! This module configures the Axum HTTP server with routing, CORS, and error handling.

use std::net::SocketAddr;
use std::time::Duration;

use axum::{
    Router,
    extract::DefaultBodyLimit,
    routing::{get, post},
};
use tower::ServiceBuilder;
use tower_http::{
    cors::{Any, CorsLayer},
    timeout::TimeoutLayer,
    trace::TraceLayer,
};

use crate::api::handlers;

/// Start the HTTP server
pub async fn start_server(
    addr: SocketAddr,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let app = create_router();

    let listener = tokio::net::TcpListener::bind(addr).await?;
    eprintln!("Server listening on {}", addr);

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
        .route(
            "/api/v1/context/verify",
            post(handlers::context_verify_handler),
        )
        .route(
            "/api/v1/context/import",
            post(handlers::context_import_handler),
        )
        .route(
            "/api/v1/extraction/domains",
            get(handlers::extraction_domains_handler),
        )
        .route(
            "/api/v1/extraction/extract",
            post(handlers::extraction_extract_handler),
        )
        .route("/api/v1/sv/init", post(handlers::sv_init_handler))
        .route("/api/v1/sv/discover", post(handlers::sv_discover_handler))
        .route(
            "/api/v1/extraction/validate",
            post(handlers::extraction_validate_handler),
        )
        .route(
            "/api/v1/context/predicates",
            post(handlers::context_predicates_handler),
        )
        .layer(
            ServiceBuilder::new()
                .layer(TraceLayer::new_for_http())
                .layer(cors)
                .layer(DefaultBodyLimit::max(1_048_576))
                .layer(TimeoutLayer::with_status_code(
                    axum::http::StatusCode::REQUEST_TIMEOUT,
                    Duration::from_secs(30),
                )),
        )
}
