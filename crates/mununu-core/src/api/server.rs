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
            "/api/v1/extraction/composition-modes",
            get(handlers::extraction_composition_modes_handler),
        )
        .route(
            "/api/v1/extraction/propose-composition",
            post(handlers::extraction_propose_composition_handler),
        )
        .route(
            "/api/v1/extraction/extract",
            post(handlers::extraction_extract_handler),
        )
        .route(
            "/api/v1/extraction/validate",
            post(handlers::extraction_validate_handler),
        )
        .route(
            "/api/v1/context/predicates",
            post(handlers::context_predicates_handler),
        )
        .route("/api/v1/templates", get(handlers::templates_handler))
        .route(
            "/api/v1/contract/validate",
            post(handlers::contract_validate_handler),
        )
        .route(
            "/api/v1/contract/discover",
            post(handlers::contract_discover_handler),
        )
        .route(
            "/api/v1/contract/query",
            post(handlers::contract_query_handler),
        )
        .route(
            "/api/v1/contract/review",
            post(handlers::contract_review_handler),
        )
        .route(
            "/api/v1/codesign/verify",
            post(handlers::codesign_verify_handler),
        )
        .route("/api/v1/verify", post(handlers::verify_project_handler))
        .route("/api/v1/btor2/cegar", post(handlers::btor2_cegar_handler))
        // Multi-engine safety portfolio — decide `bad`-reachability across every
        // sound engine (exact ⊕ native ⊕ spacer ⊕ btormc ⊕ Pono), merged under the
        // differential-oracle discipline. Surface peer of the CLI `mununu btor2 verify`.
        .route("/api/v1/btor2/verify", post(handlers::btor2_verify_handler))
        // P2 response-liveness at scale — AG(request → AF grant) via liveness-to-
        // safety + the portfolio. Surface peer of the CLI `mununu btor2 verify-liveness`.
        .route(
            "/api/v1/btor2/verify-liveness",
            post(handlers::btor2_verify_liveness_handler),
        )
        // P2 recoverability — AG EF good ("can it always get back?"), the branching
        // property SVA cannot state. Surface peer of `mununu btor2 verify-recoverability`.
        .route(
            "/api/v1/btor2/verify-recoverability",
            post(handlers::btor2_verify_recoverability_handler),
        )
        // cegar-extraction Stage 2 — SV-direct CEGAR one-call (sv2v +
        // Yosys → flattened BTOR2 → cegar_refine_loop). Surface peer of
        // the CLI `mununu sv cegar`; lets the extraction-tab SV workflow
        // run CEGAR without a manual emit-btor2-per-module step.
        .route("/api/v1/sv/cegar", post(handlers::sv_cegar_handler))
        // Track-H SVA front-end (XL.6a) — slang extracts + translates the
        // design's SVA to mu-calculus. Surface peer of `mununu sv extract-sva`
        // and the extraction-tab SVA panel. No verification (that is verify-auto).
        .route(
            "/api/v1/sv/extract-sva",
            post(handlers::sv_extract_sva_handler),
        )
        // Track-H no-sidecar verify (XL.6b) — extract SVA + verify each property
        // against the model. Surface peer of `mununu sv verify-auto` + the UI panel.
        .route(
            "/api/v1/sv/verify-auto",
            post(handlers::sv_verify_auto_handler),
        )
        .route(
            "/api/v1/verify/memory-check",
            post(handlers::memory_check_handler),
        )
        .route(
            "/api/v1/codesign/reconcile-labels",
            post(handlers::codesign_reconcile_labels_handler),
        )
        .route(
            "/api/v1/codesign/emit-chaotic-stub",
            post(handlers::codesign_emit_chaotic_stub_handler),
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
