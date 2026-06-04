// Copyright (c) Microsoft Corporation.

//! This example demonstrates how to create an HTTP client with a minimal pipeline.
//!
//! A minimal pipeline is useful in scenarios where the standard pipeline is not required
//! and the client needs to be as lightweight as possible.

use fetch::HttpClient;

#[path = "util/utils.rs"]
mod utils;

#[tokio::main]
async fn main() -> Result<(), ohno::AppError> {
    utils::init_tracing();

    // This client does not use any middleware, it's as lightweight as possible.
    let client = HttpClient::builder_tokio(fetch::tokio::TokioDeps::default())
        .minimal_pipeline()
        .build();

    let response = client.get("https://example.com").fetch().await?;

    println!("response, status code: {}", response.status());

    Ok(())
}
