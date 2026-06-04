// Copyright (c) Microsoft Corporation.

//! This example demonstrates how to customize the HTTP client pipeline.
//!
//! It uses the fake handler because the customization APIs are identical compared to a real
//! client, and it allows the example to run without the network access.

use std::time::Duration;

use fetch::HttpClient;
use fetch::fake::FakeDeps;
use fetch::resilience::timeout::HttpTimeoutLayerExt;
use http::StatusCode;
use layered::Stack;
use seatbelt::timeout::Timeout;

#[tokio::main]
async fn main() -> Result<(), ohno::AppError> {
    let client = HttpClient::builder_fake(StatusCode::OK, FakeDeps::default())
        // Use custom pipeline rather than the default one.
        // Here we add a timeout layer that wraps the dispatch handler.
        //
        // Because the fake builder is used, the dispatch handler transparently returns a
        // fake 200 OK response.
        .custom_pipeline(move |dispatch, context| {
            let stack = (
                Timeout::layer("my_timeout", context.resilience_context())
                    .http_timeout_error()
                    .timeout(Duration::from_secs(5)),
                dispatch,
            );

            stack.into_service()
        })
        .build();

    let response = client.get("https://example.com").fetch().await?;

    println!("response, status code: {}", response.status());

    Ok(())
}
