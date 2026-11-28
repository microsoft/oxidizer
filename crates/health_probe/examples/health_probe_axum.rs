// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Runs an Axum server with an HTTP readiness route.

use std::error::Error;
use std::time::Duration;

use axum::Router;
use health::Aggregator;
use health_probe::http::readiness_tower;
use observed::Sink;
use tokio::net::TcpListener;

const ANVIL_TERMINATE_AFTER: Duration = Duration::from_secs(2);

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let aggregator = Aggregator::new(Sink::noop());
    let monitor = aggregator.monitor();

    // Create components and retain their publishers in application state.

    let app = Router::new().route_service("/health/ready", readiness_tower::service(monitor));

    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    println!("listening on http://{}", listener.local_addr()?);

    let server = axum::serve(listener, app);
    if std::env::var_os("ANVIL_EXAMPLE").is_some() {
        server.with_graceful_shutdown(tokio::time::sleep(ANVIL_TERMINATE_AFTER)).await?;
    } else {
        server.await?;
    }

    Ok(())
}
