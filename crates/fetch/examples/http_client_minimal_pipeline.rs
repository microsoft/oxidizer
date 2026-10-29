// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Creates an HTTP client with the minimal pipeline.
//!
//! The minimal pipeline contains no middleware.

use fetch::HttpClient;

#[path = "util/endpoint.rs"]
mod endpoint;
#[path = "util/utils.rs"]
mod utils;

#[tokio::main]
async fn main() -> Result<(), ohno::AppError> {
    utils::init_tracing();
    let endpoint = endpoint::Endpoint::new("https://example.com").await?;

    // This client uses no middleware.
    let client = HttpClient::builder_tokio(fetch::tokio::TokioDeps::default())
        .tls_options(endpoint.rustls_options()?)
        .minimal_pipeline()
        .build();

    let response = client.get(endpoint.url.as_str()).fetch().await?;

    println!("response, status code: {}", response.status());

    endpoint.finish(1).await?;
    Ok(())
}
