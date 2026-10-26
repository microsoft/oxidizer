// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Creates an HTTP client with the minimal pipeline.
//!
//! The minimal pipeline contains no middleware.

use fetch::HttpClient;

#[path = "util/utils.rs"]
mod utils;

#[tokio::main]
async fn main() -> Result<(), ohno::AppError> {
    utils::init_tracing();

    // This client uses no middleware.
    let client = HttpClient::builder_tokio(fetch::tokio::TokioDeps::default())
        .minimal_pipeline()
        .build();

    let response = client.get("https://example.com").fetch().await?;

    println!("response, status code: {}", response.status());

    Ok(())
}
