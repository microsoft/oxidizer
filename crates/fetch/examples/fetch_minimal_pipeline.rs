// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Creates an HTTP client with the minimal pipeline.
//!
//! The minimal pipeline contains no middleware.

use fetch::HttpClient;
use observed::Sink;

#[path = "util/utils.rs"]
mod utils;

#[tokio::main]
async fn main() -> Result<(), ohno::AppError> {
    utils::init_tracing();

    // This client uses no middleware.
    // An application passes its own sink. `Sink::noop()` drops every event.
    let client = HttpClient::builder_tokio(fetch::tokio::TokioDeps::new(Sink::noop()))
        .minimal_pipeline()
        .build();

    let response = client.get("https://example.com").fetch().await?;

    println!("response, status code: {}", response.status());

    Ok(())
}
