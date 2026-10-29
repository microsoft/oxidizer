// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(missing_docs, clippy::unwrap_used, reason = "example code")]

//! Tower compatibility examples.
//!
//! The examples combine layered and Tower in three ways:
//! - Use a layered stack through the Tower interface. This requires readiness polling.
//! - Use Tower layers in a layered stack through `tower_layer()`. This does not require polling.
//! - Use layered layers in a Tower `ServiceBuilder`.

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

// Use a layered stack through the Tower interface.
async fn example_oxidizer() {
    let stack = (
        GlobalConcurrencyLimitLayer::new(1),
        Intercept::layer().on_input(|input| println!("outer input: {input}")),
        GlobalConcurrencyLimitLayer::new(1),
        Intercept::layer().on_input(|input| println!("inner input: {input}")),
        Execute::new(execute),
    );

    let mut service = stack.into_service();
    poll_fn(|cx| service.poll_ready(cx)).await.unwrap();
    service.call("hello world from Oxidizer".to_string()).await.unwrap();
}

// Use Tower layers in a layered stack.
async fn example_oxidizer_native() {
    let stack = (
        tower_layer(GlobalConcurrencyLimitLayer::new(1)),
        Intercept::layer().on_input(|input| println!("outer input: {input}")),
        tower_layer(GlobalConcurrencyLimitLayer::new(1)),
        Intercept::layer().on_input(|input| println!("inner input: {input}")),
        Execute::new(execute),
    );

    let service = stack.into_service();

    // The layered interface does not require readiness polling.
    service.execute("hello world from Oxidizer Native".to_string()).await.unwrap();
}

// Use layered layers in a Tower `ServiceBuilder`.
async fn example_tower() {
    let mut service = tower::ServiceBuilder::new()
        .concurrency_limit(10)
        .layer(Intercept::layer().on_input(|input: &String| println!("outer input: {input}")))
        .concurrency_limit(1)
        .layer(Intercept::layer().on_input(|input: &String| println!("inner input: {input}")))
        .service_fn(execute);

    // Poll the Tower service for readiness before calling it.
    poll_fn(|cx| service.poll_ready(cx)).await.unwrap();
    service.call("hello world from Tower".to_string()).await.unwrap();
}

async fn execute(data: String) -> Result<String, String> {
    println!("executing input: {data}");
    Ok::<_, String>(data)
}
