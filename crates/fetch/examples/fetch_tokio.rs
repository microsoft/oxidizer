// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Sends HTTP requests on Tokio.

use fetch::HttpClient;
use observed::Sink;

#[tokio::main]
async fn main() -> Result<(), ohno::AppError> {
    // An application passes its own sink. `Sink::noop()` drops every event.
    let client = HttpClient::new_tokio(Sink::noop());

    let response = client.get("https://example.com").fetch().await?;
    println!("Request completed with status: {}", response.status());

    Ok(())
}
