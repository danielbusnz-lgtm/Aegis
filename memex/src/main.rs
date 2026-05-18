//! Memex daemon entry point.
//!
//! Long-lived process. Ingests data from configured sources into a local store
//! and serves a query API over localhost. Clients (aegis, future tools) hit
//! the API instead of reaching out to source APIs themselves.

use std::net::SocketAddr;

const DEFAULT_ADDR: &str = "127.0.0.1:7142";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr: SocketAddr = std::env::var("MEMEX_ADDR")
        .unwrap_or_else(|_| DEFAULT_ADDR.to_string())
        .parse()?;

    eprintln!("[memex] starting daemon (scaffold) on http://{}", addr);
    let app = memex::router();
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
