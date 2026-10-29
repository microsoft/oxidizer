// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::borrow::Cow;
use std::fmt::Debug;
use std::time::Duration;

use data_privacy::RedactionEngine;
use http_extensions::routing::{BaseUriConflict, Router};
use http_extensions::{HttpBodyOptions, HttpRequest, HttpResponse};
use opentelemetry::metrics::{Meter, MeterProvider};
use seatbelt::ResilienceContext;
use thread_aware::ThreadAware;

use crate::client::HttpClientPipeline;
use crate::constants::DEFAULT_HTTP_CLIENT_NAME;
use crate::custom::{Isolation, Transport};
use crate::dispatch_builder::create_dispatch_handler;
use crate::handlers::Dispatch;
use crate::options::{ClientOptions, ConnectionKeepAlive, ConnectionPoolOptions, DecompressionOptions, Http2Options, RequestFilter};
use crate::pipeline::{CustomPipeline, Pipeline, PipelineBuilder, PipelineContext, StandardRequestPipeline};
use crate::resilience::HttpResilienceContext;
use crate::telemetry::Metering;
use crate::tls::TlsOptions;
use crate::{BaseUri, RequestHandler};

/// Configures and creates an HTTP client.
///
/// Each configuration method returns `self` for method chaining.
///
/// The default standard pipeline includes resilience, logging, and metrics. Use
/// [`minimal_pipeline`](Self::minimal_pipeline) or [`custom_pipeline`](Self::custom_pipeline) to
/// replace it.
///
/// # Construction
///
/// Use `HttpClient::builder_tokio` or `HttpClient::builder_arty` for a bundled transport.
/// Use [`custom::create_builder`][crate::custom::create_builder] for a custom transport.
#[derive(Debug, Clone)]
#[must_use]
pub struct HttpClientBuilder {
    pub(crate) options: ClientOptions,
    pipeline_builder: PipelineBuilder,
    metering: Metering,
    transport: Transport,
    resilience_context: HttpResilienceContext,
}

impl HttpClientBuilder {
    pub(super) fn new(transport: Transport) -> Self {
        let clock = transport.clock().clone();

        Self {
            options: ClientOptions::default(),
            pipeline_builder: PipelineBuilder::default(),
            metering: Metering::new(
                transport.runtime().clone(),
                transport.name().clone(),
                Cow::Borrowed(DEFAULT_HTTP_CLIENT_NAME),
            ),
            transport,
            resilience_context: HttpResilienceContext::new(&clock).name(DEFAULT_HTTP_CLIENT_NAME).use_logs(),
        }
    }

    /// Sets the name for the HTTP client.
    ///
    /// Logging and metrics use the name to identify the client. Use `snake_case`. The default name
    /// is `http_client`.
    pub fn name(mut self, name: impl Into<Cow<'static, str>>) -> Self {
        let name = name.into();
        self.metering = self.metering.with_client_name(name.clone());
        self.resilience_context = self.resilience_context.name(name);
        self
    }

    /// Allows insecure HTTP connections.
    ///
    /// The client permits only HTTPS by default. This method disables the HTTPS-only scheme
    /// filter. The bundled Tokio transport still rejects schemes other than HTTP and HTTPS. A
    /// custom transport must reject every scheme that it does not support.
    ///
    /// # Security
    ///
    /// Do not send sensitive data over HTTP. HTTP connections are not encrypted.
    pub fn insecure_allow_http(mut self) -> Self {
        self.options.transport.request_filter = RequestFilter::HttpAndHttps;
        self
    }

    /// Sets the connection timeout duration.
    ///
    /// Passes this timeout to the transport. The bundled Tokio transport applies it to name
    /// resolution and connection attempts. A custom transport must apply it. The default is 30
    /// seconds.
    pub const fn connect_timeout(mut self, timeout: Duration) -> Self {
        self.options.transport.connect_timeout = timeout;
        self
    }

    /// Configures how HTTP connections are kept alive.
    ///
    /// Keep-alive keeps active or idle connections open for later requests.
    ///
    /// By default, this value is set to [`ConnectionKeepAlive::disabled`].
    pub fn connection_keep_alive(mut self, mode: ConnectionKeepAlive) -> Self {
        self.options.transport.connection_keep_alive = mode;
        self
    }

    /// Sets the body options for every response from this client.
    ///
    /// [`HttpBodyOptions`] sets the buffer limit for
    /// [`HttpBody::into_buffered`](crate::HttpBody::into_buffered) and the idle timeout between
    /// body frames.
    ///
    /// By default, [`HttpBodyOptions::default()`] is used.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(feature = "test-util")]
    /// # {
    /// # use http_extensions::HttpBodyOptions;
    /// # use std::time::Duration;
    /// # use fetch::HttpClient;
    /// # use fetch::fake::FakeDeps;
    /// # use http::StatusCode;
    /// # let builder = HttpClient::builder_fake(StatusCode::OK, FakeDeps::default());
    /// let options = HttpBodyOptions::default()
    ///     .buffer_limit(4 * 1024 * 1024)
    ///     .timeout(Duration::from_secs(30));
    ///
    /// let client = builder.response_body_options(options).build();
    /// # }
    /// ```
    pub const fn response_body_options(mut self, options: HttpBodyOptions) -> Self {
        self.options.response_body_options = options;
        self
    }

    /// Sets formats and resource limits for automatic response decompression.
    ///
    /// Decompression is off by default. A build without a `compression-*` feature links no
    /// compression implementation. The client decompresses only formats selected by
    /// [`formats`][DecompressionOptions::formats].
    ///
    /// Requests list selected formats in `Accept-Encoding`, with the most preferred format first.
    /// Before it returns a response selected for decompression, the client removes
    /// `Content-Encoding` and `Content-Length`. Body decoding is lazy and occurs as code reads it,
    /// so a malformed body fails during that read. If a response uses a format that was not
    /// selected, the client returns it without changes.
    ///
    /// [`DecompressionFormat::ALL`][crate::options::DecompressionFormat::ALL] selects every
    /// available format. The `compression-all` feature makes all supported formats available.
    ///
    /// A format slice or array reference uses the default limits. Pass [`DecompressionOptions`] to
    /// change them.
    ///
    /// # Bounds
    ///
    /// The client uses the `http_compression` defaults. Decompressed output is limited to 64 MiB.
    /// Each decompression stage accepts at most 1,024 concatenated streams. `fetch` does not
    /// override these defaults.
    ///
    /// Explicit limits apply while code reads the body, including a streamed body. Pass `None` to
    /// [`max_output_len`][DecompressionOptions::max_output_len] to remove the output limit. A limit
    /// failure uses the `compression_limit_exceeded` error label.
    ///
    /// The [`response_body_options`][Self::response_body_options] buffer limit is separate. It
    /// applies when code collects decompressed bytes in memory. Its default is 2 GiB.
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(all(feature = "test-util", feature = "compression-gzip"))]
    /// # {
    /// # use fetch::HttpClient;
    /// # use fetch::options::{DecompressionFormat, DecompressionOptions};
    /// # use fetch::fake::FakeDeps;
    /// # use http::StatusCode;
    /// # let builder = HttpClient::builder_fake(StatusCode::OK, FakeDeps::default());
    /// let client = builder
    ///     .decompression(
    ///         DecompressionOptions::with_formats(&[DecompressionFormat::Gzip])
    ///             .max_output_len(8 * 1024 * 1024),
    ///     )
    ///     .build();
    /// # }
    /// ```
    pub fn decompression(mut self, options: impl Into<DecompressionOptions>) -> Self {
        self.options.decompression = options.into();
        self
    }

    /// Selects the minimal pipeline.
    ///
    /// This pipeline contains only [`Dispatch`]. It does not include logging, metrics, or other
    /// middleware.
    pub fn minimal_pipeline(mut self) -> Self {
        self.pipeline_builder = PipelineBuilder::Minimal;
        self
    }

    /// Replaces the standard pipeline with a custom request pipeline.
    ///
    /// The factory receives:
    ///
    /// - A [`Dispatch`] handler that handles the actual HTTP communication.
    /// - A [`PipelineContext`] with the shared pipeline dependencies.
    ///
    /// Return a middleware stack with the dispatch handler at the bottom. A layer can add logging,
    /// authentication, caching, or request and response transformations.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # use fetch::*;
    /// # use fetch::handlers::*;
    /// # use fetch::resilience::retry::{HttpRetry, HttpRetryLayerExt};
    /// # use layered::Stack;
    /// fn configure_builder(mut builder: HttpClientBuilder) -> HttpClientBuilder {
    ///     builder.custom_pipeline(move |dispatch, ctx| {
    ///         let stack = (
    ///             Logging::layer()
    ///                 .redaction_engine(ctx.redaction_engine())
    ///                 .clock(ctx.clock()),
    ///             HttpRetry::layer("my_retry", ctx.resilience_context())
    ///                 .http_configure_defaults()
    ///                 .max_retry_attempts(1),
    ///             dispatch,
    ///         );
    ///         stack.into_service()
    ///     })
    /// }
    /// ```
    pub fn custom_pipeline<F, R>(mut self, factory: F) -> Self
    where
        F: Fn(Dispatch, PipelineContext) -> R + Send + Sync + 'static,
        R: RequestHandler + 'static,
    {
        self.pipeline_builder = PipelineBuilder::Custom(CustomPipeline::new(factory));
        self
    }

    /// Sets the TLS options.
    ///
    /// Use `TlsOptions::builder_rustls()` for rustls. Use
    /// `TlsOptions::builder_native_tls()` for native TLS. Both builders support mutual TLS (`mTLS`)
    /// through `client_identity`. The native TLS builder also supports certificate server-name
    /// overrides for individual endpoints.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// # #[cfg(feature = "rustls")]
    /// # {
    /// # use fetch::tls::TlsOptions;
    /// # use fetch::HttpClientBuilder;
    /// # fn example(builder: HttpClientBuilder) {
    /// let client = builder
    ///     .tls_options(TlsOptions::builder_rustls().build())
    ///     .build();
    /// # }
    /// # }
    /// ```
    pub fn tls_options(mut self, tls_options: TlsOptions) -> Self {
        self.options.tls = tls_options;
        self
    }

    /// Sets the HTTP versions that the client supports.
    ///
    /// The default versions are HTTP/1.1 and HTTP/2.
    pub fn supported_http_versions(mut self, versions: &[http::Version]) -> Self {
        self.options.transport.supported_http_versions = versions.to_vec();
        self
    }

    /// Sets a custom OpenTelemetry meter provider for the client.
    ///
    /// The client uses the global meter provider by default. This method sets a provider for this
    /// client.
    ///
    /// The client creates the scoped meter when [`build`][Self::build] runs. A
    /// [`name`][Self::name] set before or after this call appears in the metrics.
    ///
    /// # Performance
    ///
    /// For a thread-isolated runtime, use a per-thread meter provider to avoid contention on the
    /// global provider.
    ///
    /// [`MeterProvider`]: https://docs.rs/opentelemetry/latest/opentelemetry/metrics/trait.MeterProvider.html
    #[cfg_attr(test, mutants::skip)] // Mutants remove fields that the public behavior cannot distinguish.
    pub fn meter_provider<P: MeterProvider + Send + Sync + 'static>(mut self, meter_provider: P) -> Self {
        // Use the provider for both resilience and HTTP metrics.
        self.resilience_context = self.resilience_context.use_metrics(&meter_provider);
        self.metering = self.metering.with_provider(meter_provider);
        self
    }

    /// Changes the standard pipeline.
    ///
    /// The closure receives the current pipeline and returns the changed pipeline. The standard
    /// pipeline includes resilience, logging, and metrics.
    ///
    /// See [`StandardRequestPipeline`] for the defaults.
    ///
    /// # Multiple Calls
    ///
    /// Consecutive calls are additive. Each closure receives the result of the previous call.
    ///
    /// If another method selects a different pipeline type, the next call starts with a new default
    /// [`StandardRequestPipeline`].
    ///
    /// # Example
    ///
    /// ```rust
    /// # use std::time::Duration;
    /// # use fetch::{HttpClient, HttpClientBuilder};
    /// # fn configure_builder(mut builder: HttpClientBuilder) {
    /// let client = builder
    ///     .standard_pipeline(|pipeline, _context| {
    ///         // Set the attempt timeout to 5 seconds.
    ///         pipeline.attempt_timeout(|timeout| timeout.timeout(Duration::from_secs(5)))
    ///     })
    ///     .build();
    /// # }
    /// ```
    pub fn standard_pipeline<F>(self, configure: F) -> Self
    where
        F: Fn(StandardRequestPipeline, PipelineContext) -> StandardRequestPipeline + Send + Sync + 'static,
    {
        Self {
            pipeline_builder: self.pipeline_builder.configure_standard(configure),
            ..self
        }
    }

    /// Sets the base URI for the client.
    ///
    /// This setting overrides an endpoint in the request [`Uri`](templated_uri::Uri):
    ///
    /// - If this method sets a [`BaseUri`], the client ignores the request [`BaseUri`].
    /// - Otherwise, the client uses the request [`BaseUri`].
    /// - If neither source has a base URI, validation returns [`HttpError`](crate::HttpError).
    ///
    /// ```rust
    /// # #[cfg(feature = "test-util")]
    /// # {
    /// # use http::StatusCode;
    /// # use fetch::fake::FakeHandler;
    /// # use fetch::HttpClient;
    /// # use fetch::HttpResponseBuilder;
    /// # use fetch::fake::FakeDeps;
    /// # use templated_uri::BaseUri;
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let client = HttpClient::builder_fake(FakeHandler::default(), FakeDeps::default())
    ///     .base_uri(BaseUri::from_static("https://example.com"))
    ///     .build();
    ///
    /// let response = client.get("/foo/bar").fetch().await?;
    /// # Ok(())
    /// # }
    /// # }
    /// ```
    pub fn base_uri(self, base_uri: impl Into<BaseUri>) -> Self {
        // Preserve historical semantics: the client's base URI overrides any endpoint
        // already present on the request URI.
        self.router(Router::fixed(base_uri.into()).conflict_policy(BaseUriConflict::UseRouted))
    }

    /// Sets the [`Router`] that resolves a destination [`BaseUri`] for each request.
    ///
    /// A router can provide a primary endpoint and fallback endpoints. If it has alternatives, the
    /// standard pipeline enables recovery from connection-unavailable errors, so a later attempt
    /// can target another endpoint.
    ///
    /// This method and [`HttpClientBuilder::base_uri`] set the same configuration. The last call
    /// wins. `base_uri(uri)` uses `Router::fixed(uri)` with
    /// `BaseUriConflict::UseRouted`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # fn main() {
    /// # #[cfg(feature = "test-util")] {
    /// # use fetch::HttpClient;
    /// # use fetch::fake::FakeHandler;
    /// # use fetch::fake::FakeDeps;
    /// # use http_extensions::routing::Router;
    /// # use templated_uri::BaseUri;
    /// let client = HttpClient::builder_fake(FakeHandler::default(), FakeDeps::default())
    ///     .router(Router::fallback(
    ///         BaseUri::from_static("https://primary.example.com/"),
    ///         BaseUri::from_static("https://secondary.example.com/"),
    ///     ))
    ///     .build();
    /// # }
    /// # }
    /// ```
    pub fn router(mut self, router: Router) -> Self {
        self.options.router = router;
        self
    }

    /// Sets the HTTP/2 options.
    ///
    /// These options apply only to connections that negotiate HTTP/2.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # use fetch::HttpClient;
    /// # use fetch::options::Http2Options;
    /// # fn configure_builder(mut builder: fetch::HttpClientBuilder) {
    /// let client = builder
    ///     .http2_options(Http2Options::default().initial_max_send_streams(100))
    ///     .build();
    /// # }
    /// ```
    pub fn http2_options(mut self, options: Http2Options) -> Self {
        self.options.transport.http_2 = options;
        self
    }

    /// Sets the connection pool options.
    ///
    /// The pool reuses connections instead of establishing a connection for each request.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # use std::time::Duration;
    /// # use fetch::HttpClient;
    /// # use fetch::options::ConnectionPoolOptions;
    /// # fn configure_builder(mut builder: fetch::HttpClientBuilder) {
    /// let client = builder
    ///     .connection_pool_options(
    ///         ConnectionPoolOptions::default()
    ///             .max_connections(50)
    ///             .connection_idle_timeout(Duration::from_secs(300)),
    ///     )
    ///     .build();
    /// # }
    /// ```
    pub fn connection_pool_options(mut self, options: ConnectionPoolOptions) -> Self {
        self.options.transport.connection_pool = options;
        self
    }

    /// Sets the redaction engine for the client.
    ///
    /// Logging and telemetry use the [`RedactionEngine`] to remove sensitive request and response
    /// data.
    pub fn redaction_engine(mut self, redaction_engine: &RedactionEngine) -> Self {
        self.options.redaction_engine = redaction_engine.clone();
        self
    }

    /// Builds the configured [`HttpClient`](crate::HttpClient).
    ///
    /// The returned client uses every setting on this builder.
    #[must_use]
    pub fn build(self) -> crate::HttpClient {
        let clock = self.transport.clock().clone();
        let router = self.options.router.clone();
        let aware = Aware {
            transport: self.transport,
            pipeline: self.pipeline_builder,
            options: self.options,
            resilience_context: self.resilience_context,
            metering: self.metering,
        };
        let body_builder = aware.transport.create_body_builder(&aware.options);
        let pipeline = match aware.transport.isolation() {
            Isolation::Isolated => HttpClientPipeline::Isolated(performables::arc::Arc::new_with_data(aware, Aware::into_pipeline)),
            Isolation::Shared => HttpClientPipeline::Shared(performables::arc::Arc::new(aware.into_pipeline())),
        };

        crate::HttpClient::new(pipeline, body_builder, clock, router)
    }
}

#[derive(Debug, Clone, ThreadAware)]
struct Aware {
    #[thread_aware(skip)]
    metering: Metering,
    pipeline: PipelineBuilder,
    #[thread_aware(skip)]
    options: ClientOptions,
    transport: Transport,
    resilience_context: ResilienceContext<HttpRequest, crate::Result<HttpResponse>>,
}

impl Aware {
    fn into_pipeline(self) -> Pipeline {
        let meter: Meter = self.metering.into();
        let body_builder = self.transport.create_body_builder(&self.options);
        let dispatch = create_dispatch_handler(&meter, self.options.clone(), &self.transport, &body_builder);
        let context = PipelineContext::new(
            self.resilience_context,
            &meter,
            self.options.redaction_engine,
            body_builder,
            self.transport.clock().clone(),
            self.options.router,
        );

        self.pipeline.build(dispatch, context)
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use http::StatusCode;
    use http_extensions::{HttpBodyBuilder, HttpBodyOptions};

    use crate::fake::FakeDeps;
    use crate::options::{ConnectionIdleTimeout, ConnectionPoolOptions, Http2Options};
    use crate::{HttpClient, HttpClientBuilder};

    static_assertions::assert_impl_all!(HttpClientBuilder: Send, Sync, Clone);

    #[test]
    fn standard_pipeline_customization_applied() {
        let client = HttpClient::builder_fake(StatusCode::OK, FakeDeps::default())
            .standard_pipeline(|pipeline, _context| pipeline.retry(|retry| retry.max_retry_attempts(5)))
            .build();

        let dbg = client.pipeline().debug_string();
        assert!(dbg.contains("max_attempts: 6"));
    }

    #[test]
    fn connection_keep_alive_sets_option() {
        use std::time::Duration;

        use crate::options::ConnectionKeepAlive;

        let builder = HttpClient::builder_fake(StatusCode::OK, FakeDeps::default()).connection_keep_alive(
            ConnectionKeepAlive::active_connections(Duration::from_secs(15), Duration::from_secs(5)),
        );

        assert!(matches!(
            builder.options.transport.connection_keep_alive,
            ConnectionKeepAlive::ActiveConnections { interval, timeout }
                if interval == Duration::from_secs(15) && timeout == Duration::from_secs(5)
        ));
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn tls_options_are_stored_without_breaking_the_pipeline() {
        use crate::tls::TlsOptions;

        // The fake transport ignores TLS. Storing custom options must not affect requests.
        let client = HttpClient::builder_fake(StatusCode::OK, FakeDeps::default())
            .tls_options(TlsOptions::default())
            .build();

        let response = client.get("https://example.com").fetch().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[test]
    fn redaction_engine_sets_option() {
        use data_privacy::simple_redactor::{SimpleRedactor, SimpleRedactorMode};
        use data_privacy::{RedactedToString, RedactionEngine};
        use templated_uri::{PathAndQuery, Uri};

        let engine = RedactionEngine::builder()
            .add_class_redactor(Uri::DATA_CLASS, SimpleRedactor::with_mode(SimpleRedactorMode::Passthrough))
            .build();

        let builder = HttpClient::builder_fake(StatusCode::OK, FakeDeps::default()).redaction_engine(&engine);

        // A passthrough redactor leaves the path unchanged.
        let redacted = PathAndQuery::from_static("/path").to_redacted_string(&builder.options.redaction_engine);
        assert_eq!(redacted, "/path");
    }

    #[test]
    fn standard_pipeline_after_minimal_creates_new_pipeline() {
        let client = HttpClient::builder_fake(StatusCode::OK, FakeDeps::default())
            .standard_pipeline(|pipeline, _context| pipeline.retry(|retry| retry.max_retry_attempts(3)))
            .minimal_pipeline()
            .standard_pipeline(|pipeline, _context| pipeline)
            .build();

        let dbg = client.pipeline().debug_string();
        assert!(dbg.contains("max_attempts: 4"));
    }

    #[test]
    fn response_body_options() {
        let custom = HttpBodyOptions::default().buffer_limit(1234);

        let mut builder = HttpClient::builder_fake(StatusCode::OK, FakeDeps::default());
        assert_eq!(builder.options.response_body_options, HttpBodyOptions::default());

        builder = builder.response_body_options(custom);
        assert_eq!(custom, builder.options.response_body_options);

        let client = builder.build();
        let builder: &HttpBodyBuilder = client.as_ref();

        assert!(format!("{builder:?}").contains("1234"));
    }

    #[test]
    fn test_http2_options_configuration() {
        let builder = HttpClient::builder_fake(StatusCode::OK, FakeDeps::default())
            .http2_options(Http2Options::default().initial_max_send_streams(100).adaptive_window(true));
        assert_eq!(builder.options.transport.http_2.initial_max_send_streams, Some(100));

        assert!(builder.options.transport.http_2.adaptive_window);
        assert_eq!(builder.options.transport.http_2.initial_stream_window_size, None);

        let builder = HttpClient::builder_fake(StatusCode::OK, FakeDeps::default())
            .http2_options(Http2Options::default().initial_stream_window_size(1024 * 1024));
        assert_eq!(builder.options.transport.http_2.initial_stream_window_size, Some(1_048_576));
    }

    #[test]
    fn test_connection_pool_options_configuration() {
        use std::time::Duration;

        let builder = HttpClient::builder_fake(StatusCode::OK, FakeDeps::default()).connection_pool_options(
            ConnectionPoolOptions::default()
                .max_connections(50)
                .connection_idle_timeout(Duration::from_mins(5)),
        );

        assert_eq!(builder.options.transport.connection_pool.max_connections, 50);
        match builder.options.transport.connection_pool.connection_idle_timeout {
            ConnectionIdleTimeout::Limited(duration) => {
                assert_eq!(duration, Duration::from_mins(5));
            }
            ConnectionIdleTimeout::Unlimited => panic!("Expected Limited variant"),
        }
    }

    #[test]
    fn test_http2_and_connection_pool_options_chaining() {
        use std::time::Duration;

        let builder = HttpClient::builder_fake(StatusCode::OK, FakeDeps::default())
            .http2_options(Http2Options::default().initial_max_send_streams(200))
            .connection_pool_options(
                ConnectionPoolOptions::default()
                    .max_connections(25)
                    .connection_idle_timeout(Duration::from_mins(2)),
            );

        // Test the HTTP/2 options.
        assert_eq!(builder.options.transport.http_2.initial_max_send_streams, Some(200));

        // Test the connection pool options.
        assert_eq!(builder.options.transport.connection_pool.max_connections, 25);
        match builder.options.transport.connection_pool.connection_idle_timeout {
            ConnectionIdleTimeout::Limited(duration) => {
                assert_eq!(duration, Duration::from_mins(2));
            }
            ConnectionIdleTimeout::Unlimited => panic!("Expected Limited variant"),
        }
    }

    #[test]
    fn defaults_ok() {
        // Create a builder with default settings.
        let _builder = HttpClient::builder_fake(StatusCode::OK, FakeDeps::default());
    }

    #[test]
    fn name_ok() {
        // Set the client name.
        let _builder = HttpClient::builder_fake(StatusCode::OK, FakeDeps::default()).name("custom_client");
    }

    #[test]
    fn test_clone_produces_isolated_instances() {
        use std::time::Duration;

        // Create a configured builder.
        let builder_1 = HttpClient::builder_fake(StatusCode::OK, FakeDeps::default())
            .name("base_client")
            .connect_timeout(Duration::from_secs(10))
            .response_body_options(HttpBodyOptions::default().buffer_limit(1000));

        // Clone the builder.
        let mut builder_2 = builder_1.clone();

        // Both builders start with the same values.
        assert_eq!(
            builder_1.options.transport.connect_timeout,
            builder_2.options.transport.connect_timeout
        );
        assert_eq!(builder_1.options.response_body_options, builder_2.options.response_body_options);

        // Change the cloned builder.
        builder_2 = builder_2
            .name("cloned_client")
            .connect_timeout(Duration::from_secs(30))
            .response_body_options(HttpBodyOptions::default().buffer_limit(5000))
            .http2_options(Http2Options::default().initial_max_send_streams(100))
            .connection_pool_options(
                ConnectionPoolOptions::default()
                    .max_connections(50)
                    .connection_idle_timeout(Duration::from_mins(5)),
            );

        // The original builder does not change.
        assert_eq!(builder_1.options.transport.connect_timeout, Duration::from_secs(10));
        assert_eq!(
            builder_1.options.response_body_options,
            HttpBodyOptions::default().buffer_limit(1000)
        );
        assert_eq!(builder_1.options.transport.http_2.initial_max_send_streams, None);
        assert_eq!(builder_1.options.transport.connection_pool.max_connections, usize::MAX);

        // The cloned builder has the new values.
        assert_eq!(builder_2.options.transport.connect_timeout, Duration::from_secs(30));
        assert_eq!(
            builder_2.options.response_body_options,
            HttpBodyOptions::default().buffer_limit(5000)
        );
        assert_eq!(builder_2.options.transport.http_2.initial_max_send_streams, Some(100));
        assert_eq!(builder_2.options.transport.connection_pool.max_connections, 50);
        match builder_2.options.transport.connection_pool.connection_idle_timeout {
            ConnectionIdleTimeout::Limited(duration) => {
                assert_eq!(duration, Duration::from_mins(5));
            }
            ConnectionIdleTimeout::Unlimited => panic!("Expected Limited variant"),
        }
    }

    #[test]
    fn multiple_pools_sets_options() {
        use crate::options::{ConnectionPoolOptions, PoolSelection};

        let builder = HttpClient::builder_fake(StatusCode::OK, FakeDeps::default()).connection_pool_options(
            ConnectionPoolOptions::default().multiple_pools(5, PoolSelection::saturating(PoolSelection::DEFAULT_REQUESTS_PER_CLIENT)),
        );

        let multiple_pools = builder.options.transport.connection_pool.multiple_pools;
        let Some((pool_count, _selection)) = multiple_pools else {
            panic!("expected multiple pools to be configured");
        };
        assert_eq!(pool_count, 5);
    }

    #[cfg_attr(miri, ignore)] // SdkMeterProvider uses operations unsupported by Miri.
    #[test]
    fn meter_provider_updates_all_fields() {
        let provider = opentelemetry_sdk::metrics::SdkMeterProvider::default();

        let builder = HttpClient::builder_fake(StatusCode::OK, FakeDeps::default());
        assert!(!builder.metering.has_custom_provider());

        let builder = builder.meter_provider(provider);
        assert!(builder.metering.has_custom_provider());
    }
}
