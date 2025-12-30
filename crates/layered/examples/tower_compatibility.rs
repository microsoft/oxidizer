// Copyright (c) Microsoft Corporation.

//! Tower Compatibility Example
//!
//! Demonstrates three approaches to service composition:
//!
//! 1. Pure Oxidizer execution stack with Tower execution model: Used when you want to produce
//!    Tower services but prefer Oxidizer's middleware composition model. Note that this approach
//!    requires polling the service for readiness before calling it.
//!
//! 2. Oxidizer with `tower_layer()` adapter: Used when you want to use Tower layers but prefer
//!    Oxidizer's native execution model without the need for polling.
//!
//! 3. Tower `ServiceBuilder` with Oxidizer layers: Used when you want or are required\
//!    to use Tower's `ServiceBuilder` for middleware composition.

use std::future::poll_fn;

use layered::prelude::*;
use layered::tower::tower_layer;
use layered::{Execute, Intercept};
use tower::limit::GlobalConcurrencyLimitLayer;
use tower_service::Service as TowerService;

#[tokio::main]
async fn main() {
    println!("=== Oxidizer with Tower Adapter Example ===");
    example_oxidizer().await;

    println!("\n=== Oxidizer Native Example ===");
    example_oxidizer_native().await;

    println!("\n=== Tower ServiceBuilder Example ===");
    example_tower().await;
}

// Oxidizer execution stack with Tower layers (requires polling before Tower service can be called)
async fn example_oxidizer() {
    let execution_stack = (
        GlobalConcurrencyLimitLayer::new(1),
        Intercept::layer().on_input(|input| println!("outer input: {input}")),
        GlobalConcurrencyLimitLayer::new(1),
        Intercept::layer().on_input(|input| println!("inner input: {input}")),
        Execute::new(execute),
    );

    let mut service = execution_stack.build();
    poll_fn(|cx| service.poll_ready(cx)).await.unwrap();
    service.call("hello world from Oxidizer".to_string()).await.unwrap();
}

// Oxidizer execution stack with tower_layer() adapter (no polling needed)
async fn example_oxidizer_native() {
    let execution_stack = (
        tower_layer(GlobalConcurrencyLimitLayer::new(1)),
        Intercept::layer().on_input(|input| println!("outer input: {input}")),
        tower_layer(GlobalConcurrencyLimitLayer::new(1)),
        Intercept::layer().on_input(|input| println!("inner input: {input}")),
        Execute::new(execute),
    );

    let service = execution_stack.build();

    // Direct execution - no polling required
    service.execute("hello world from Oxidizer Native".to_string()).await.unwrap();
}

// Tower ServiceBuilder with Oxidizer layers
async fn example_tower() {
    let mut service = tower::ServiceBuilder::new()
        .concurrency_limit(10)
        .layer(Intercept::layer().on_input(|input: &String| println!("outer input: {input}")))
        .concurrency_limit(1)
        .layer(Intercept::layer().on_input(|input: &String| println!("inner input: {input}")))
        .service_fn(execute);

    // Tower services must be polled for readiness first
    poll_fn(|cx| service.poll_ready(cx)).await.unwrap();
    service.call("hello world from Tower".to_string()).await.unwrap();
}

async fn execute(data: String) -> Result<String, String> {
    println!("executing input: {data}");
    Ok::<_, String>(data)
}
