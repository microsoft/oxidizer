// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Plugs a custom `EchoHandler` into [`fetch::custom::create_builder`] as the transport
//! handler. Every request's body is returned verbatim in the response.

use bytesbuf::mem::GlobalPool;
use fetch::custom::{CustomDeps, Isolation, create_builder};
use fetch::{HttpRequest, HttpResponse, HttpResponseBuilder};
use http::StatusCode;
use http_extensions::HttpBodyBuilder;
use layered::Service;
use tick::Clock;

#[tokio::main]
async fn main() -> Result<(), ohno::AppError> {
    let deps = CustomDeps {
        clock: Clock::new_tokio(),
        global_pool: GlobalPool::new(),
        extras: (),
    };

    let client = create_builder(
        |cx| EchoHandler {
            body_builder: cx.body_builder,
        },
        Isolation::Shared,
        deps,
    )
    .insecure_allow_http()
    .build();

    let payload = "Hello, transport handler!";
    let response = client.post("http://example.com/echo").text(payload).fetch_text().await?;

    println!("status: {}", response.status());
    let echoed = response.into_body();
    println!("echoed body: {echoed}");
    assert_eq!(echoed, payload);

    Ok(())
}

/// Transport handler that echoes the request body back to the caller.
struct EchoHandler {
    body_builder: HttpBodyBuilder,
}

impl Service<HttpRequest> for EchoHandler {
    type Out = http_extensions::Result<HttpResponse>;

    async fn execute(&self, input: HttpRequest) -> Self::Out {
        let echoed = input.into_body().into_bytes().await?;

        HttpResponseBuilder::new(&self.body_builder)
            .status(StatusCode::OK)
            .bytes(echoed)
            .build()
    }
}
