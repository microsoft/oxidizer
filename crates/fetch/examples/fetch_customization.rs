// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Changes resilience layers in the standard HTTP client pipeline.

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

    // Use a fake handler to simulate request failures.
    let fake_handler = FakeHandler::from_fn(|req| {
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
                // Increase the total timeout.
                .total_timeout(|timeout| timeout.timeout(Duration::from_mins(1)))
                // Change the retry behavior.
                .retry(|retry| {
                    retry
                        .base_delay(Duration::ZERO)
                        .max_retry_attempts(50) // The fake handler performs no network I/O.
                        .http_clone(HttpClone::all()) // Clone requests for every HTTP method.
                        .http_recovery(|response: &HttpResponse| response.recovery())
                })
                // Decrease the attempt timeout.
                .attempt_timeout(|timeout| timeout.timeout(Duration::from_secs(2)))
                // Inspect each request attempt.
                .attempt_intercept(|intercept| {
                    intercept.modify_input(|req| {
                        // The callback can inspect or change the request.
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
