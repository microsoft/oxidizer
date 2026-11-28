// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Uses `test-util` to return specified HTTP responses.

use fetch::fake::FakeHandler;
use fetch::{HttpClient, HttpResponseBuilder};
use http::StatusCode;
use observed::Sink;

#[tokio::main]
async fn main() -> Result<(), ohno::AppError> {
    let fake_handler = FakeHandler::from_fn(|req| {
        println!("fake handler called for request, url: {}", req.uri());

        HttpResponseBuilder::new_fake()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .text("fake text")
            .build()
    });

    // An application passes its own sink. `Sink::noop()` drops every event.
    let client = HttpClient::new_fake(fake_handler, Sink::noop());

    let response = client.get("https://example.com").fetch_text().await?;

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(response.into_body(), "fake text");

    Ok(())
}
