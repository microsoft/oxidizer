// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Replaces the standard HTTP client pipeline.
//!
//! The fake handler uses the same pipeline APIs as a network transport and requires no network
//! access.

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
        // Add a timeout layer around the dispatch handler.
        //
        // The fake dispatch handler returns 200 OK.
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
