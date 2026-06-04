// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Test-only constructors for [`HttpClient`].
//!
//! These factory methods produce HTTP clients backed by a fake handler so tests can exercise
//! request flows without making real network calls. They are gated behind the `test-util`
//! feature (and unconditionally available in test builds).
//!
//! The [`FakeHandler`] used to produce canned responses is re-exported here for convenience.

#[doc(no_inline)]
pub use http_extensions::FakeHandler;
use thread_aware::ThreadAware;
use tick::Clock;

use crate::custom::{CustomContext, CustomDeps, Isolation};
use crate::handlers::TransportHandler;
use crate::{HttpClient, HttpClientBuilder};

/// Configuration dependencies for fake/test HTTP operations.
///
/// Minimal configuration used in testing environments where only basic
/// clock functionality is needed.
#[derive(Debug, Clone, ThreadAware)]
pub struct FakeDeps {
    /// Clock for testing time-based operations.
    pub clock: Clock,
}

impl Default for FakeDeps {
    fn default() -> Self {
        Self {
            clock: tick::ClockControl::new().into(),
        }
    }
}

impl From<&Clock> for FakeDeps {
    #[cfg_attr(test, mutants::skip)] // Mutations using a wrong clock will easily lead to timeouts.
    fn from(clock: &Clock) -> Self {
        Self { clock: clock.clone() }
    }
}

impl HttpClient {
    /// Creates a builder for a test-friendly HTTP client with a fake handler.
    ///
    /// Unlike [`HttpClient::new_fake`], this method returns a builder that can be further
    /// customized before constructing the client. Use this when you need more control
    /// over the client's configuration in test scenarios.
    ///
    /// # Examples
    ///
    /// ```
    /// # use std::time::Duration;
    /// # use fetch::HttpClient;
    /// # use http::StatusCode;
    /// # use fetch::fake::FakeDeps;
    /// async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let client = HttpClient::builder_fake(StatusCode::NOT_FOUND, FakeDeps::default())
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
    /// Available only when compiled with the `test-util` feature.
    pub fn builder_fake(handler: impl Into<FakeHandler>, deps: impl Into<FakeDeps>) -> HttpClientBuilder {
        let deps = deps.into();
        let handler = handler.into();

        // Re-layer on top of the in-crate `builder_custom_internal` path. The
        // `FakeHandler` travels through `CustomDeps::extras` and is cloned
        // into a fresh `TransportHandler` for every connection pool slot.
        Self::builder_custom_internal(
            move |cx: CustomContext<FakeHandler>| TransportHandler::new(cx.extras),
            Isolation::Shared,
            CustomDeps {
                clock: deps.clock,
                global_pool: bytesbuf::mem::GlobalPool::new(),
                extras: handler,
            },
        )
    }

    /// Creates a test-friendly HTTP client that uses mock responses.
    ///
    /// This factory method provides a convenient way to create a client for testing without
    /// making real network requests. It automatically configures the builder with test-friendly
    /// defaults like allowing HTTP and using a minimal pipeline.
    ///
    /// # Examples
    ///
    /// ```
    /// # use fetch::HttpClient;
    /// # use http::StatusCode;
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// // Create a client that always returns a specific status code
    /// let client = HttpClient::new_fake(StatusCode::OK);
    ///
    /// // Now you can use this client in tests without real network requests
    /// let response = client.get("https://example.com").fetch().await?;
    /// assert_eq!(response.status(), StatusCode::OK);
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// Available only when compiled with the `test-util` feature.
    pub fn new_fake(handler: impl Into<FakeHandler>) -> Self {
        Self::builder_fake(handler, FakeDeps::default())
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

    use super::FakeDeps;
    use crate::HttpClient;
    use crate::pipeline::Pipeline;

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn ctor_fake_ok() {
        let client = HttpClient::new_fake(StatusCode::INTERNAL_SERVER_ERROR);

        let response = client.get("http://example.com").fetch().await.unwrap();

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert!(matches!(client.pipeline(), Pipeline::Minimal(_)));
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn test_builder_fake_with_custom_options() {
        let client = HttpClient::builder_fake(StatusCode::IM_A_TEAPOT, FakeDeps::default())
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
        let _client = HttpClient::builder_fake(FakeHandler::never_completes(), FakeDeps::default())
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

        let _client = HttpClient::builder_fake(FakeHandler::never_completes(), &clock)
            .custom_pipeline(|root, ctx| {
                let dbg = format!("{:?}", ctx.clock());
                assert!(dbg.contains("kind: \"controlled\""));
                root
            })
            .build();
    }
}
