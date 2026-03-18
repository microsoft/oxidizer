// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! An example of a custom HTTP client using `http_extensions`.
//!
//! This example demonstrates how to create a simple HTTP client that just echoes back the
//! data it receives.

use bytesbuf::mem::GlobalPool;
use http_extensions::{HttpBodyBuilder, HttpRequest, HttpRequestBuilderExt, HttpResponse, HttpResponseBuilder, StatusExt};
use layered::Service;

#[tokio::main]
async fn main() -> Result<(), ohno::AppError> {
    // Create a custom client that implements the Service trait
    let client = CustomClient::default();

    // Use the client to send a request
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

/// The implementation of `AsRef<HttpBodyBuilder>` allows us to use the
/// `RequestBuilder` extension methods provided by `http_extensions`.
impl AsRef<HttpBodyBuilder> for CustomClient {
    fn as_ref(&self) -> &HttpBodyBuilder {
        &self.builder
    }
}

impl Default for CustomClient {
    fn default() -> Self {
        Self {
            builder: HttpBodyBuilder::new(GlobalPool::new()),
        }
    }
}

impl Service<HttpRequest> for CustomClient {
    type Out = http_extensions::Result<HttpResponse>;

    async fn execute(&self, input: HttpRequest) -> Self::Out {
        let data = input.into_body().into_bytes().await?;

        // echo back the data we received
        HttpResponseBuilder::new(&self.builder).status(200).bytes(data).build()
    }
}
