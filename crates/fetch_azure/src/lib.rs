// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(all(coverage_nightly, test), feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/fetch_azure/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/fetch_azure/favicon.ico")]

//! Use [`fetch`] as the HTTP transport for the Azure SDK for Rust.
//!
//! The Azure SDK abstracts its HTTP transport behind the
//! [`typespec_client_core::http::HttpClient`] trait. This crate provides
//! [`FetchHttpClient`], an adapter that implements that trait on top of a
//! [`fetch::HttpClient`], so Azure SDK pipelines can run over `fetch` and
//! benefit from its resilience, observability, and runtime features.
//!
//! # Example
//!
//! ```
//! use std::sync::Arc;
//!
//! use fetch::HttpClient;
//! use fetch_azure::FetchHttpClient;
//! use typespec_client_core::http::HttpClient as AzureHttpClient;
//!
//! // Wrap an existing `fetch` client so it can be handed to the Azure SDK.
//! fn as_azure_transport(client: HttpClient) -> Arc<dyn AzureHttpClient> {
//!     Arc::new(FetchHttpClient::new(client))
//! }
//! # let _ = as_azure_transport;
//! ```

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use bytesbuf::BytesView;
use futures::{StreamExt as _, TryStreamExt as _};
use layered::Service as _;
use typespec_client_core::error::{Error, ErrorKind};
use typespec_client_core::http::headers::{HeaderName, HeaderValue, Headers};
use typespec_client_core::http::request::{Body, Request};
use typespec_client_core::http::response::PinnedStream;
use typespec_client_core::http::{AsyncRawResponse, HttpClient};

/// An [`HttpClient`] that uses a [`fetch::HttpClient`] as its transport.
///
/// Construct one from an existing `fetch` client with [`FetchHttpClient::new`]
/// (or via [`From`]) and pass it to the Azure SDK wherever a
/// `dyn HttpClient` is expected. See [`new_http_client`] for a convenience that
/// returns an `Arc<dyn HttpClient>` directly.
#[derive(Debug, Clone)]
pub struct FetchHttpClient {
    client: fetch::HttpClient,
}

impl FetchHttpClient {
    /// Creates a new adapter that forwards requests to the given `fetch` client.
    #[must_use]
    pub const fn new(client: fetch::HttpClient) -> Self {
        Self { client }
    }

    /// Returns a reference to the wrapped [`fetch::HttpClient`].
    #[must_use]
    pub const fn inner(&self) -> &fetch::HttpClient {
        &self.client
    }

    /// Consumes the adapter and returns the wrapped [`fetch::HttpClient`].
    #[must_use]
    pub fn into_inner(self) -> fetch::HttpClient {
        self.client
    }

    /// Converts a typespec [`Request`] into a `fetch` request.
    fn to_fetch_request(&self, request: &Request) -> typespec_client_core::Result<fetch::HttpRequest> {
        // `Method::as_str` yields a canonical token (e.g. "GET") that `fetch`'s
        // builder parses into an `http::Method`; this avoids matching on the
        // `#[non_exhaustive]` typespec `Method` enum.
        let mut builder = self.client.request(request.method().as_str(), request.url().as_str());

        for (name, value) in request.headers().iter() {
            builder = builder.header(name.as_str(), value.as_str());
        }

        builder.body(self.to_fetch_body(request.body())).build().map_err(|error| {
            Error::with_error(
                ErrorKind::DataConversion,
                error,
                "failed to convert the Azure request into a fetch request",
            )
        })
    }

    /// Converts a typespec request [`Body`] into a `fetch` [`HttpBody`](fetch::HttpBody).
    ///
    /// Empty byte bodies reuse a shared empty body, and non-empty byte bodies are
    /// wrapped without copying. Seekable streams are forwarded as a chunk stream.
    fn to_fetch_body(&self, body: &Body) -> fetch::HttpBody {
        let builder: &fetch::HttpBodyBuilder = self.client.as_ref();

        match body {
            Body::Bytes(bytes) if bytes.is_empty() => builder.empty(),
            Body::Bytes(bytes) => builder.bytes(BytesView::from(bytes.clone())),
            Body::SeekableStream(stream) => {
                let stream = stream.clone().map(|chunk| {
                    chunk
                        .map(BytesView::from)
                        .map_err(|error| fetch::HttpError::unavailable(format!("failed to read the Azure request body: {error}")))
                });
                builder.stream(stream, &fetch::options::HttpBodyOptions::default())
            }
        }
    }
}

impl From<fetch::HttpClient> for FetchHttpClient {
    fn from(client: fetch::HttpClient) -> Self {
        Self::new(client)
    }
}

/// Wraps a [`fetch::HttpClient`] as an `Arc<dyn HttpClient>`.
///
/// This is a convenience for the common case of handing a `fetch`-backed
/// transport to the Azure SDK.
#[must_use]
pub fn new_http_client(client: fetch::HttpClient) -> Arc<dyn HttpClient> {
    Arc::new(FetchHttpClient::new(client))
}

#[async_trait]
impl HttpClient for FetchHttpClient {
    async fn execute_request(&self, request: &Request) -> typespec_client_core::Result<AsyncRawResponse> {
        let request = self.to_fetch_request(request)?;

        let response = self
            .client
            .execute(request)
            .await
            .map_err(|error| Error::with_error(ErrorKind::Io, error, "the fetch HTTP client failed to execute the request"))?;

        Ok(to_async_raw_response(response))
    }
}

/// Converts a `fetch` [`HttpResponse`](fetch::HttpResponse) into an [`AsyncRawResponse`].
fn to_async_raw_response(response: fetch::HttpResponse) -> AsyncRawResponse {
    let (parts, body) = response.into_parts();
    let status = parts.status.as_u16().into();
    let headers = to_headers(&parts.headers);

    let body = body
        .into_stream()
        .map_ok(|view| view.to_bytes())
        .map_err(|error| Error::with_error(ErrorKind::Io, error, "failed to read the response body"));
    let body: PinnedStream = Box::pin(body);

    AsyncRawResponse::new(status, headers, body)
}

/// Converts an [`http::HeaderMap`] into [`Headers`].
///
/// Header values that are not valid UTF-8 are skipped, mirroring the behavior
/// of the built-in `reqwest` transport in the Azure SDK.
fn to_headers(map: &http::HeaderMap) -> Headers {
    let headers = map
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (HeaderName::from(name.as_str().to_owned()), HeaderValue::from(value.to_owned())))
        })
        .collect::<HashMap<_, _>>();

    Headers::from(headers)
}
