// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! This example demonstrates how to customize the HTTP client. Standard pipeline and resilience
//! in particular.

use std::time::Duration;

use fetch::fake::{FakeDeps, FakeHandler};
use fetch::resilience::HttpClone;
use fetch::resilience::retry::HttpRetryLayerExt;
use fetch::{HttpClient, HttpResponse, HttpResponseBuilder, StatusExt};
use http::StatusCode;

#[path = "util/utils.rs"]
mod utils;

#[tokio::main]
async fn main() -> Result<(), ohno::AppError> {
    utils::init_tracing();

    // Simulate issues with the request by using a fake handler.
    let fake_handler = FakeHandler::from_sync_handler(|req| {
        let status_code = match fastrand::u32(0..10) {
            0..5 => StatusCode::INTERNAL_SERVER_ERROR,
            _ => StatusCode::OK,
        };

        HttpResponseBuilder::new_fake()
            .status(status_code)
            .text(format!("response from {}", req.uri()))
            .build()
    });

    let client = HttpClient::builder_fake(fake_handler, FakeDeps::default())
        .standard_pipeline(|pipeline, _context| {
            pipeline
                // Increase the total timeout for the pipeline.
                .total_timeout(|timeout| timeout.timeout(Duration::from_mins(1)))
                // Customize the retry behavior.
                .retry(|retry| {
                    retry
                        .base_delay(Duration::ZERO)
                        .max_retry_attempts(50) // we can do many retries, this does not do any external IO
                        .http_clone(HttpClone::all()) // we clone all idempotent requests (PUT, DELETE, etc)
                        .http_recovery(|response: &HttpResponse| response.recovery())
                })
                // Decrease the attempt timeout.
                .attempt_timeout(|timeout| timeout.timeout(Duration::from_secs(2)))
                // Optionally, we could also customize the attempt_intercept callback.
                .attempt_intercept(|intercept| {
                    intercept.modify_input(|req| {
                        // You can inspect/modify the request here.
                        println!("attempting request to {}", req.uri());
                        req
                    })
                })
        })
        .build();

    let text = client.get("https://example.com").fetch_text_body().await?;

    println!("response: {text}");

    Ok(())
}
