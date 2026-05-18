// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! An example of a custom HTTP client using `http_extensions`.
//!
//! This example demonstrates how to create a simple HTTP client that just echoes back the
//! data it receives. The client uses the [`Router`] feature to set a base URI, so that
//! request builders can use relative paths.

use bytesbuf::mem::GlobalPool;
use http_extensions::routing::{BaseUriConflict, Router, RouterContext};
use http_extensions::{HttpBodyBuilder, HttpRequest, HttpRequestBuilderExt, HttpResponse, HttpResponseBuilder, StatusExt};
use layered::Service;
use templated_uri::BaseUri;
use tick::Clock;

#[tokio::main]
async fn main() -> Result<(), ohno::AppError> {
    // Create a custom client that implements the Service trait, configured with a base URI.
    // The client uses the `Router` feature internally to attach the base URI to requests.
    let client = CustomClient::new(BaseUri::from_static("http://localhost:8080"));

    // Use the client to send a request, providing only the relative path: the base URI is
    // attached by the client's router.
    let response = client
        .request_builder()
        .get("/hello-world")
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
    router: Router,
}

/// The implementation of `AsRef<HttpBodyBuilder>` allows us to use the
/// `RequestBuilder` extension methods provided by `http_extensions`.
impl AsRef<HttpBodyBuilder> for CustomClient {
    fn as_ref(&self) -> &HttpBodyBuilder {
        &self.builder
    }
}

impl CustomClient {
    fn new(base_uri: BaseUri) -> Self {
        Self {
            builder: HttpBodyBuilder::new(GlobalPool::new(), &Clock::new_tokio()),
            router: Router::fixed(base_uri).conflict_policy(BaseUriConflict::Fail),
        }
    }
}

impl Service<HttpRequest> for CustomClient {
    type Out = http_extensions::Result<HttpResponse>;

    async fn execute(&self, mut input: HttpRequest) -> Self::Out {
        // Resolve the request's URI through the configured router, attaching the base URI
        // to the relative path provided by the caller.
        self.router.update_request_uri(RouterContext::new(), &mut input)?;

        println!("request uri: {}", input.uri());

        let data = input.into_body().into_bytes().await?;

        // echo back the data we received
        HttpResponseBuilder::new(&self.builder).status(200).bytes(data).build()
    }
}
