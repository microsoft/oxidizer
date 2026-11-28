// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Runs an Axum server with an HTTP readiness route.

use std::error::Error;

use axum::Router;
use health::Aggregator;
use health_probe::http::readiness_tower;
use observed::Sink;
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let aggregator = Aggregator::new(Sink::noop());
    let monitor = aggregator.monitor();

    // Create components and retain their publishers in application state.

    let app = Router::new().route_service("/health/ready", readiness_tower::service(monitor));

    let listener = TcpListener::bind("0.0.0.0:8080").await?;
    axum::serve(listener, app).await?;

    Ok(())
}
