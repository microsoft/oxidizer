// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(all(coverage_nightly, test), feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/fetch_azure/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/fetch_azure/favicon.ico")]

//! Bundle [`fetch`] and [`anyspawn`] as Azure SDK abstractions.
//!
//! The Azure SDK abstracts its HTTP transport behind the
//! [`azure_core::http::HttpClient`] trait and its task spawning, sleeping, and
//! yielding behind the [`azure_core::async_runtime::AsyncRuntime`] trait. This
//! crate provides adapters for both:
//!
//! - [`AzureHttpClient`] implements [`HttpClient`] on top of a
//!   [`fetch::HttpClient`], so Azure SDK pipelines run over `fetch` and benefit
//!   from its resilience and observability.
//! - [`SpawnerRuntime`] implements [`AsyncRuntime`] on top of an
//!   [`anyspawn::Spawner`], so the Azure SDK spawns and sleeps on the runtime of
//!   your choice.
//!
//! # Example
//!
//! ```
//! use std::sync::Arc;
//!
//! use anyspawn::Spawner;
//! use azure_core::async_runtime::{AsyncRuntime, set_async_runtime};
//! use azure_core::http::HttpClient;
//! use fetch::HttpClient as FetchClient;
//! use fetch_azure::{AzureHttpClient, new_async_runtime};
//! use tick::Clock;
//!
//! // Adapt a `fetch` client into an Azure SDK transport.
//! fn transport(client: FetchClient) -> Arc<dyn HttpClient> {
//!     AzureHttpClient::from(client).into()
//! }
//!
//! // Install an `anyspawn`-backed async runtime (sleeping on a `tick::Clock`).
//! fn install_runtime(spawner: Spawner, clock: Clock) {
//!     let runtime: Arc<dyn AsyncRuntime> = new_async_runtime(spawner, clock);
//!     let _ = set_async_runtime(runtime);
//! }
//! # let _ = (transport, install_runtime);
//! ```

use std::collections::HashMap;
use std::future::ready;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll};

use anyspawn::{JoinHandle, Spawner};
use async_trait::async_trait;
use azure_core::async_runtime::{AbortableTask, AsyncRuntime, SpawnedTask, TaskFuture};
use azure_core::error::{Error, ErrorKind};
use azure_core::http::headers::{HeaderName, HeaderValue, Headers};
use azure_core::http::request::{Body, Request};
use azure_core::http::response::PinnedStream;
use azure_core::http::{AsyncRawResponse, HttpClient};
use azure_core::time::Duration;
use bytesbuf::BytesView;
use futures::{StreamExt as _, TryStreamExt as _};
use layered::Service as _;
use tick::Clock;

/// An [`HttpClient`] that uses a [`fetch::HttpClient`] as its transport.
///
/// Construct one from an existing `fetch` client with [`AzureHttpClient::new`]
/// (or via [`From`]), then convert it into an `Arc<dyn HttpClient>` via [`From`]
/// / [`Into`] to hand to the Azure SDK:
///
/// ```
/// # use std::sync::Arc;
/// # use azure_core::http::HttpClient;
/// # use fetch_azure::AzureHttpClient;
/// # fn wrap(client: fetch::HttpClient) -> Arc<dyn HttpClient> {
/// AzureHttpClient::from(client).into()
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct AzureHttpClient {
    client: fetch::HttpClient,
}

impl AzureHttpClient {
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
    fn to_fetch_request(&self, request: &Request) -> azure_core::Result<fetch::HttpRequest> {
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

impl From<fetch::HttpClient> for AzureHttpClient {
    fn from(client: fetch::HttpClient) -> Self {
        Self::new(client)
    }
}

impl From<AzureHttpClient> for Arc<dyn HttpClient> {
    fn from(client: AzureHttpClient) -> Self {
        Arc::new(client)
    }
}

#[async_trait]
impl HttpClient for AzureHttpClient {
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

/// An [`AsyncRuntime`] that spawns work on an [`anyspawn::Spawner`] and sleeps
/// on a [`tick::Clock`].
///
/// Construct one from an existing [`Spawner`] and [`Clock`] with
/// [`SpawnerRuntime::new`] (or via [`From`]) and install it as the Azure SDK
/// runtime with [`azure_core::async_runtime::set_async_runtime`]. See
/// [`new_async_runtime`] for a convenience that returns an
/// `Arc<dyn AsyncRuntime>` directly.
#[derive(Debug, Clone)]
pub struct SpawnerRuntime {
    spawner: Spawner,
    clock: Clock,
}

impl SpawnerRuntime {
    /// Creates a new runtime that spawns work on `spawner` and sleeps on `clock`.
    #[must_use]
    pub const fn new(spawner: Spawner, clock: Clock) -> Self {
        Self { spawner, clock }
    }

    /// Returns a reference to the wrapped [`Spawner`].
    pub const fn spawner(&self) -> &Spawner {
        &self.spawner
    }

    /// Returns a reference to the wrapped [`Clock`].
    #[must_use]
    pub const fn clock(&self) -> &Clock {
        &self.clock
    }
}

impl From<(Spawner, Clock)> for SpawnerRuntime {
    fn from((spawner, clock): (Spawner, Clock)) -> Self {
        Self::new(spawner, clock)
    }
}

/// Wraps an [`anyspawn::Spawner`] and [`tick::Clock`] as an `Arc<dyn AsyncRuntime>`.
///
/// This is a convenience for installing a `fetch`-friendly runtime with
/// [`azure_core::async_runtime::set_async_runtime`].
#[must_use]
pub fn new_async_runtime(spawner: Spawner, clock: Clock) -> Arc<dyn AsyncRuntime> {
    Arc::new(SpawnerRuntime::new(spawner, clock))
}

impl AsyncRuntime for SpawnerRuntime {
    fn spawn(&self, f: TaskFuture) -> SpawnedTask {
        Box::pin(SpawnerTask::new(self.spawner.spawn(f)))
    }

    fn sleep(&self, duration: Duration) -> TaskFuture {
        let clock = self.clock.clone();
        Box::pin(async move {
            // `time::Duration` can be negative; clamp such values to zero.
            let duration = std::time::Duration::try_from(duration).unwrap_or_default();
            clock.delay(duration).await;
        })
    }

    fn yield_now(&self) -> TaskFuture {
        std::thread::yield_now();
        Box::pin(ready(()))
    }
}

/// Adapts an [`anyspawn::JoinHandle`] into an [`AbortableTask`].
struct SpawnerTask {
    handle: JoinHandle<()>,
    aborted: AtomicBool,
}

impl SpawnerTask {
    fn new(handle: JoinHandle<()>) -> Self {
        Self {
            handle,
            aborted: AtomicBool::new(false),
        }
    }
}

impl Future for SpawnerTask {
    type Output = Result<(), Box<dyn std::error::Error + Send>>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        if this.aborted.load(Ordering::Acquire) {
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut this.handle).poll(cx).map(Ok)
    }
}

impl AbortableTask for SpawnerTask {
    fn abort(&self) {
        // `anyspawn` join handles cannot cancel the underlying task, so mark the
        // task aborted and resolve on the next poll. The spawned work may keep
        // running, but the caller is no longer blocked on it.
        self.aborted.store(true, Ordering::Release);
    }
}
