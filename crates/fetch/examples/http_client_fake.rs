// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! This example shows how to use the "fakes" feature to mock the HTTP client with specific responses.

use fetch::fake::FakeHandler;
use fetch::{HttpClient, HttpResponseBuilder};
use http::StatusCode;

#[tokio::main]
async fn main() -> Result<(), ohno::AppError> {
    let fake_handler = FakeHandler::from_sync_handler(|req| {
        println!("fake handler called for request, url: {}", req.uri());

        HttpResponseBuilder::new_fake()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .text("fake text")
            .build()
    });

    let client = HttpClient::new_fake(fake_handler);

    let response = client.get("https://example.com").fetch_text().await?;

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(response.into_body(), "fake text");

    Ok(())
}
