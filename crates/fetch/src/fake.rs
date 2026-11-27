// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Test-only constructors for [`HttpClient`].
//!
//! These methods create clients that use a fake handler without network I/O. They require
//! `test-util` outside test builds.
//!
//! This module re-exports [`FakeHandler`] for canned responses.

use bytesbuf::mem::{GlobalPool, OpaqueMemory};
#[doc(no_inline)]
pub use http_extensions::FakeHandler;
use observed::Sink;
use thread_aware::ThreadAware;
use tick::{Clock, ClockControl};

use crate::custom::{CustomContext, CustomDeps, Isolation};
use crate::handlers::TransportHandler;
use crate::{HttpClient, HttpClientBuilder};

/// Dependencies for fake HTTP operations.
///
/// Fake clients require a clock and an `observed` sink.
#[derive(Debug, Clone, ThreadAware)]
pub struct FakeDeps {
    /// Clock for testing time-based operations.
    pub clock: Clock,
    /// Sink for `observed` telemetry.
    pub sink: Sink,
}

impl FakeDeps {
    /// Creates `FakeDeps` with the clock of a new [`ClockControl`] and the given sink.
    #[must_use]
    pub fn new(sink: Sink) -> Self {
        Self::with_clock(&ClockControl::new().to_clock(), sink)
    }

    /// Creates `FakeDeps` with the given clock and sink.
    #[must_use]
    pub fn with_clock(clock: &Clock, sink: Sink) -> Self {
        Self {
            clock: clock.clone(),
            sink,
        }
    }
}

impl HttpClient {
    /// Creates an HTTP client builder with a fake handler.
    ///
    /// Unlike [`HttpClient::new_fake`], this method returns a builder for more configuration.
    ///
    /// # Examples
    ///
    /// ```
    /// # use std::time::Duration;
    /// # use fetch::HttpClient;
    /// # use http::StatusCode;
    /// # use fetch::fake::FakeDeps;
    /// # use observed::Sink;
    /// async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// // An application passes its own sink. `Sink::noop()` drops every event.
    /// let client = HttpClient::builder_fake(StatusCode::NOT_FOUND, FakeDeps::new(Sink::noop()))
    ///     .connect_timeout(Duration::from_millis(100))
    ///     .insecure_allow_http()
    ///     .build();
    ///
    /// let response = client.get("http://test-url.local").fetch().await?;
    /// assert_eq!(response.status(), StatusCode::NOT_FOUND);
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// This method requires `test-util` outside test builds.
    pub fn builder_fake(handler: impl Into<FakeHandler>, deps: impl Into<FakeDeps>) -> HttpClientBuilder {
        let deps = deps.into();
        let handler = handler.into();

        // Pass FakeHandler through CustomDeps::extras and clone it for each pool slot.
        Self::builder_custom_internal(
            crate::constants::FAKE_RUNTIME_NAME,
            crate::constants::FAKE_TRANSPORT_NAME,
            move |cx: CustomContext<FakeHandler>, _client_attributes| TransportHandler::new(cx.extras),
            Isolation::Shared,
            CustomDeps {
                clock: deps.clock,
                memory: OpaqueMemory::new(GlobalPool::new()),
                sink: deps.sink,
                extras: handler,
            },
        )
    }

    /// Creates an HTTP client that returns fake responses.
    ///
    /// The client permits HTTP and uses the minimal pipeline. It performs no network I/O.
    ///
    /// # Examples
    ///
    /// ```
    /// # use fetch::HttpClient;
    /// # use http::StatusCode;
    /// # use observed::Sink;
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// // An application passes its own sink. `Sink::noop()` drops every event.
    /// // Always return one status code.
    /// let client = HttpClient::new_fake(StatusCode::OK, Sink::noop());
    ///
    /// // Send a request without network I/O.
    /// let response = client.get("https://example.com").fetch().await?;
    /// assert_eq!(response.status(), StatusCode::OK);
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// This method requires `test-util` outside test builds.
    pub fn new_fake(handler: impl Into<FakeHandler>, sink: Sink) -> Self {
        Self::builder_fake(handler, FakeDeps::new(sink))
            .insecure_allow_http()
            .minimal_pipeline()
            .build()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::time::Duration;

    use http::StatusCode;
    use http_extensions::FakeHandler;
    use observed::Sink;
    use tick::Clock;

    use super::FakeDeps;
    use crate::HttpClient;
    use crate::pipeline::Pipeline;

    static_assertions::assert_not_impl_any!(FakeDeps: Default, From<&'static Clock>);

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn ctor_fake_ok() {
        let client = HttpClient::new_fake(StatusCode::INTERNAL_SERVER_ERROR, Sink::noop());

        let response = client.get("http://example.com").fetch().await.unwrap();

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert!(matches!(client.pipeline(), Pipeline::Minimal(_)));
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn test_builder_fake_with_custom_options() {
        let client = HttpClient::builder_fake(StatusCode::IM_A_TEAPOT, FakeDeps::new(Sink::noop()))
            .connect_timeout(Duration::from_millis(100))
            .insecure_allow_http()
            .minimal_pipeline()
            .build();

        let response = client.get("http://test-url.local").fetch().await.unwrap();
        assert_eq!(response.status(), StatusCode::IM_A_TEAPOT);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn fake_builder_no_clock() {
        let _client = HttpClient::builder_fake(FakeHandler::never_completes(), FakeDeps::new(Sink::noop()))
            .custom_pipeline(|root, ctx| {
                let dbg = format!("{:?}", ctx.clock());
                assert!(dbg.contains("kind: \"controlled\""));
                root
            })
            .build();
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn fake_builder_custom_clock() {
        let clock = tick::ClockControl::new().auto_advance(Duration::from_secs(2)).to_clock();

        let _client = HttpClient::builder_fake(FakeHandler::never_completes(), FakeDeps::with_clock(&clock, Sink::noop()))
            .custom_pipeline(|root, ctx| {
                let dbg = format!("{:?}", ctx.clock());
                assert!(dbg.contains("kind: \"controlled\""));
                root
            })
            .build();
    }
}
