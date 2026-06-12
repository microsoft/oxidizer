// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Adapts a Tokio-based [`fetch::HttpClient`] into an Azure SDK transport and
//! issues a request through the [`typespec_client_core::http::HttpClient`] trait.
//!
//! Run with: `cargo run --example azure_transport`

use std::sync::Arc;

use fetch::HttpClient as FetchClient;
use fetch_azure::new_http_client;
use typespec_client_core::http::{HttpClient, Method, Request, Url};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Build a `fetch` client (Tokio runtime + rustls TLS) and adapt it so it can
    // be used wherever the Azure SDK expects an `Arc<dyn HttpClient>` transport.
    let transport: Arc<dyn HttpClient> = new_http_client(FetchClient::new_tokio());

    // In a real application you would hand `transport` to an Azure SDK client's
    // options. Here we drive it directly to show the round-trip.
    let request = Request::new(Url::parse("https://example.com")?, Method::Get);
    let response = transport.execute_request(&request).await?;

    println!("request completed with status: {}", u16::from(response.status()));

    Ok(())
}
