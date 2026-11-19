// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! This example defines a custom HTTP client with `http_extensions`.
//!
//! The client returns the data that it receives.

use bytesbuf::mem::GlobalPool;
use http_extensions::{HttpBodyBuilder, HttpRequest, HttpRequestBuilderExt, HttpResponse, HttpResponseBuilder, StatusExt};
use layered::Service;
use tick::Clock;

#[tokio::main]
async fn main() -> Result<(), ohno::AppError> {
    // Create a custom client that implements the Service trait.
    let client = CustomClient::default();

    // Send a request through the client.
    let response = client
        .request_builder()
        .get("http://localhost:8080")
        .text("Hello!")
        .fetch_text()
        .await?
        .ensure_success()?;

    println!("response received, status {}, body: {}", response.status(), response.body());

    Ok(())
}

#[derive(Debug)]
struct CustomClient {
    builder: HttpBodyBuilder,
}

/// This implementation enables the request builder extension methods from `http_extensions`.
impl AsRef<HttpBodyBuilder> for CustomClient {
    fn as_ref(&self) -> &HttpBodyBuilder {
        &self.builder
    }
}

impl Default for CustomClient {
    fn default() -> Self {
        Self {
            builder: HttpBodyBuilder::new(GlobalPool::new(), &Clock::new_tokio()),
        }
    }
}

impl Service<HttpRequest> for CustomClient {
    type Out = http_extensions::Result<HttpResponse>;

    async fn execute(&self, input: HttpRequest) -> Self::Out {
        let data = input.into_body().into_bytes().await?;

        // Return the received data.
        HttpResponseBuilder::new(&self.builder).status(200).bytes(data).build()
    }
}
