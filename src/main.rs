//! Entry point of CrashLend: builds the protocol and starts the HTTP server.

use crashlend::api::{self, AppState};
use crashlend::protocol::Protocol;
use crashlend::types::{Params, Price};

/// Address the server listens on.
const ADDRESS: &str = "127.0.0.1:3000";

#[tokio::main]
async fn main() {
    // Starting price of ETH, before any scenario is replayed.
    let protocol = Protocol::new(Params::default(), Price::from_usdc(2_000));
    let app = api::router(AppState::new(protocol));

    let listener = tokio::net::TcpListener::bind(ADDRESS)
        .await
        .expect("cannot bind the server address");
    println!("CrashLend listening on http://{ADDRESS}");
    axum::serve(listener, app).await.expect("server error");
}
