// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Azure request body adaptation through an in-process fetch fake.

use std::sync::{Arc, Mutex};

use azure_core::Bytes;
use azure_core::http::request::Request;
use azure_core::http::{HttpClient as _, Method, Url};
use fetch::fake::FakeHandler;

#[test]
fn gamma_survivor_empty_azure_bytes_use_empty_fetch_body_variant() {
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&bodies);
    let handler = FakeHandler::from_fn(move |request| {
        recorded.lock().unwrap().push(format!("{:?}", request.body()));
        fetch::HttpResponseBuilder::new_fake().build()
    });
    let fetch = fetch::HttpClient::builder_fake(handler, &tick::Clock::new_frozen())
        .minimal_pipeline()
        .build();
    let client = fetch_azure::HttpClient::new(fetch);
    for bytes in [Bytes::new(), Bytes::from_static(b"x")] {
        let mut request = Request::new(Url::parse("https://example.invalid/path").unwrap(), Method::Post);
        request.set_body(bytes);
        futures::executor::block_on(client.execute_request(&request)).unwrap();
    }
    assert_eq!(
        *bodies.lock().unwrap(),
        [
            "HttpBody { kind: Empty, options: HttpBodyOptions { timeout: None, buffer_limit: None } }",
            "HttpBody { kind: Bytes, options: HttpBodyOptions { timeout: None, buffer_limit: None } }",
        ]
    );
}
