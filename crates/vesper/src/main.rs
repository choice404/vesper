//! The vesper binary. It wires logging to stderr, since stdout carries the
//! protocol, then hands control to the server loop.

use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    let filter = EnvFilter::try_from_env("VESPER_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(filter)
        .init();

    vesper::run().await;
}
