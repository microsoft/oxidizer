// Copyright (c) Microsoft Corporation.

//! Tower interoperability examples.
//!
//! Shows three ways to combine layered and Tower services:
//! - Layered stack with Tower execution (requires polling)
//! - Layered stack with `tower_layer()` adapter (no polling)
//! - Tower `ServiceBuilder` with layered layers

use std::future::poll_fn;

use layered::prelude::*;
use layered::tower::tower_layer;
use layered::{Execute, Intercept};
use tower::limit::GlobalConcurrencyLimitLayer;
use tower_service::Service as TowerService;

#[tokio::main]
async fn main() {
    println!("=== Layered with Tower Execution ===");
    example_oxidizer().await;

    println!("\n=== Layered with tower_layer() ===");
    example_oxidizer_native().await;

    println!("\n=== Tower ServiceBuilder ===");
    example_tower().await;
}

// Layered stack with Tower execution model (requires polling)
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

// Layered stack with tower_layer() adapter (no polling needed)
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

// Tower ServiceBuilder with layered layers
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
