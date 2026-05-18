//! Memex: personal data layer for aegis.
//!
//! Eventual responsibilities:
//!   * Ingest connectors (Gmail today, Slack/Linear/code/photos later)
//!   * Local SQLite store as the source of truth
//!   * Vector embeddings + semantic search alongside keyword search
//!   * HTTP API over localhost for clients (aegis voice, future CLIs, dashboards)
//!
//! Today this is a scaffold. The router is wired up; the actual stores and
//! ingest workers are TODO.

use axum::{Json, Router, routing::get, routing::post};
use serde::{Deserialize, Serialize};

pub fn router() -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/search", post(search))
        .route("/recent/{source}", get(recent))
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
    })
}

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
    version: &'static str,
}

#[derive(Deserialize)]
pub struct SearchRequest {
    pub query: String,
    pub sources: Option<Vec<String>>,
    pub limit: Option<usize>,
}

async fn search(Json(_req): Json<SearchRequest>) -> Json<Vec<SearchHit>> {
    // TODO: embed the query, run hybrid search over the local store.
    Json(vec![])
}

async fn recent(axum::extract::Path(_source): axum::extract::Path<String>) -> Json<Vec<SearchHit>> {
    // TODO: return the most recent N items from a given source.
    Json(vec![])
}

#[derive(Serialize)]
struct SearchHit {
    source: String,
    id: String,
    title: String,
    snippet: String,
    score: f32,
}
