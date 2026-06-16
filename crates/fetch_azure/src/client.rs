// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The [`HttpClient`] transport adapter.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use azure_core::error::{Error, ErrorKind};
use azure_core::http::headers::{HeaderName, HeaderValue, Headers};
use azure_core::http::request::{Body, Request};
use azure_core::http::response::PinnedStream;
use azure_core::http::{AsyncRawResponse, HttpClient as HttpClientTrait};
use bytesbuf::BytesView;
use futures::{StreamExt as _, TryStreamExt as _};
use layered::Service as _;

/// An [`azure_core::http::HttpClient`] implementation backed by a
/// [`fetch::HttpClient`] transport.
///
/// Construct one from an existing `fetch` client with [`HttpClient::new`] (or
/// via [`From`]), then convert it into the `Arc<dyn azure_core::http::HttpClient>`
/// that Azure SDK clients accept as their transport. The Azure SDK shares its
/// transport as an `Arc` to promote reuse: one client (and its connection pool)
/// is shared across the request pipeline rather than cloned per call. The same
/// intent underlies the `Policy` and `TokenCredential` traits.
///
/// ```
/// # use std::sync::Arc;
/// # use azure_core::http::HttpClient as HttpClientTrait;
/// # use fetch_azure::HttpClient;
/// # fn example(client: fetch::HttpClient) {
/// let transport: Arc<dyn HttpClientTrait> = HttpClient::from(client).into();
/// # let _ = transport;
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct HttpClient {
    client: fetch::HttpClient,
}

impl HttpClient {
    /// Creates a new adapter that forwards requests to the given `fetch` client.
    #[must_use]
    pub const fn new(client: fetch::HttpClient) -> Self {
        Self { client }
    }

    /// Converts an Azure SDK [`Request`] into a `fetch` request.
    fn to_fetch_request(&self, request: &Request) -> azure_core::Result<fetch::HttpRequest> {
        // `Method::as_str` yields a canonical token (e.g. "GET") that `fetch`'s
        // builder parses into an `http::Method`; this avoids matching on the
        // `#[non_exhaustive]` Azure SDK `Method` enum.
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

    /// Converts an Azure SDK request [`Body`] into a `fetch` [`HttpBody`](fetch::HttpBody).
    ///
    /// Empty byte bodies reuse a shared empty body, and non-empty byte bodies are
    /// wrapped without copying. Seekable streams are forwarded as a chunk stream.
    // The empty-body fast path yields a body that is observationally identical to
    // the general bytes path (both report a zero-length body), so the
    // `is_empty()` guard is an equivalent mutant that no test can distinguish.
    #[cfg_attr(test, mutants::skip)]
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

impl From<fetch::HttpClient> for HttpClient {
    fn from(client: fetch::HttpClient) -> Self {
        Self::new(client)
    }
}

impl From<HttpClient> for Arc<dyn HttpClientTrait> {
    fn from(client: HttpClient) -> Self {
        Arc::new(client)
    }
}

#[async_trait]
impl HttpClientTrait for HttpClient {
    async fn execute_request(&self, request: &Request) -> azure_core::Result<AsyncRawResponse> {
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
