// Copyright (c) Microsoft Corporation.

//! An example of how the resilience of HTTP client can be customized.

use std::time::Duration;

use fetch::fake::{FakeDeps, FakeHandler};
use fetch::resilience::retry::HttpRetryLayerExt;
use fetch::{HttpClient, HttpResponse, StatusExt};
use http::StatusCode;
use seatbelt::retry::Backoff;

#[path = "util/utils.rs"]
mod utils;

#[tokio::main]
async fn main() -> Result<(), ohno::AppError> {
    utils::init_tracing();

    let fake_handler =
        FakeHandler::from_status_codes([StatusCode::INTERNAL_SERVER_ERROR, StatusCode::INTERNAL_SERVER_ERROR, StatusCode::OK]);

    let client = HttpClient::builder_fake(fake_handler, FakeDeps::default())
        .standard_pipeline(|pipeline, _| {
            // customize the retry resilience middleware
            pipeline.retry(|retry| {
                retry
                    .http_recovery(|response: &HttpResponse| response.recovery())
                    .max_retry_attempts(4)
                    .base_delay(Duration::ZERO)
                    .backoff(Backoff::Constant)
            })
        })
        .build();

    let response = client.get("https://www.example.com").fetch().await?;

    println!("response: {}", response.status());

    Ok(())
}
