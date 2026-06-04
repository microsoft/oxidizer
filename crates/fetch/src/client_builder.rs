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
use crate::handlers::{Dispatch, DispatchMode};
use crate::options::{ClientOptions, ConnectionKeepAlive, ConnectionPoolOptions, Http2Options, PoolIndex, RequestFilter};
use crate::pipeline::{CustomPipelineFactory, Pipeline, PipelineBuilder, PipelineContext, StandardRequestPipeline};
use crate::resilience::HttpResilienceContext;
use crate::telemetry::Metering;
use crate::tls::TlsOptions;
use crate::{BaseUri, RequestHandler};

/// Builder for creating and configuring an HTTP client.
///
/// This builder follows the builder pattern, allowing you to customize
/// various aspects of the [`HttpClient`](super::HttpClient) before creating it.
/// Each configuration method returns `self` for method chaining.
///
/// By default, the builder is configured to use a standard pipeline that includes
/// resilience features and observability (logging, metrics).
/// This behavior can be modified with methods like [`minimal_pipeline`](Self::minimal_pipeline)
/// or [`custom_pipeline`](Self::custom_pipeline).
///
/// # Construction
///
/// The builder instance is created using the `HttpClient::builder_tokio` method, or
/// the free-standing [`custom::create_builder`][crate::custom::create_builder] function
/// for a custom transport.
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
            metering: Metering::Global,
            transport,
            resilience_context: HttpResilienceContext::new(&clock).name(DEFAULT_HTTP_CLIENT_NAME).use_logs(),
        }
    }

    /// Sets the name for the HTTP client.
    ///
    /// The name is used in logging and metrics to identify the HTTP client instance. The name should
    /// follow the `snake_case` convention. By default, the client is named "`http_client`".
    pub fn name(mut self, name: impl Into<Cow<'static, str>>) -> Self {
        self.resilience_context = self.resilience_context.name(name);
        self
    }

    /// Allows insecure HTTP connections.
    ///
    /// By default, the client only permits HTTPS connections. This method enables
    /// both HTTP and HTTPS requests. Use this for testing or internal networks only.
    ///
    /// # Security
    ///
    /// HTTP connections are unencrypted and can be intercepted. Use with caution!
    pub fn insecure_allow_http(mut self) -> Self {
        self.options.transport.request_filter = RequestFilter::HttpAndHttps;
        self
    }

    /// Sets the connection timeout duration.
    ///
    /// This sets how long to wait for a connection to be established before giving up.
    /// If not specified, a default value of 30 seconds will be used.
    pub const fn connect_timeout(mut self, timeout: Duration) -> Self {
        self.options.transport.connect_timeout = timeout;
        self
    }

    /// Configures how HTTP connections are kept alive.
    ///
    /// Keep-alive maintains open or idle connections, reducing latency for subsequent requests
    /// by avoiding the overhead of establishing new TCP connections.
    ///
    /// By default, this value is set to [`ConnectionKeepAlive::disabled`].
    pub fn connection_keep_alive(mut self, mode: ConnectionKeepAlive) -> Self {
        self.options.transport.connection_keep_alive = mode;
        self
    }

    /// Sets the body options applied to every response produced by this client.
    ///
    /// [`HttpBodyOptions`] controls body-level policies such as the buffer
    /// limit (maximum memory used when buffering via
    /// [`HttpBody::into_buffered`](crate::HttpBody::into_buffered)) and the
    /// idle timeout between body frames.
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

    /// Enables the minimal pipeline mode for the client.
    ///
    /// In this mode, the client uses only the [`Dispatch`] handler directly without any middleware,
    /// giving you complete control but without features like logging or metrics. This is useful
    /// when you need a streamlined client with minimal overhead.
    pub fn minimal_pipeline(mut self) -> Self {
        self.pipeline_builder = PipelineBuilder::Minimal;
        self
    }

    /// Configures the client to use a custom request pipeline instead of the default standard pipeline.
    ///
    /// By default, the HTTP client uses a standard pipeline that includes common middleware
    /// for handling requests and responses. This method allows you to replace that pipeline
    /// with your own implementation, giving you complete control over request processing.
    ///
    /// The factory function receives:
    /// - A [`Dispatch`] handler that handles the actual HTTP communication.
    /// - A [`PipelineContext`] with additional context information.
    ///
    /// In your callback, you can provide your own stack of middleware with the dispatch handler at the bottom.
    /// Each middleware can add functionality like logging, authentication, caching, or custom
    /// request/response transformations. The middleware forms a chain where each one can process
    /// the request before passing it to the next handler in the chain.
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
    ///             Logging::layer(ctx.clock(), ctx.redaction_engine()),
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
        self.pipeline_builder = PipelineBuilder::Custom(CustomPipelineFactory::new(factory));
        self
    }

    /// Sets TLS options for this client.
    ///
    /// Use `TlsOptions::builder_rustls()` for the rustls backend,
    /// or `TlsOptions::builder_native_tls()` for the platform native TLS backend.
    /// The rustls backend also supports mutual TLS (`mTLS`) via the builder's
    /// `client_identity` method.
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

    /// Sets the supported HTTP versions for the client.
    ///
    /// The default is HTTP/1.1 and HTTP/2. This method allows you to change which
    /// HTTP protocol versions the client will use when connecting to servers.
    pub fn supported_http_versions(mut self, versions: &[http::Version]) -> Self {
        self.options.transport.supported_http_versions = versions.to_vec();
        self
    }

    /// Sets a custom OpenTelemetry meter provider for the client.
    ///
    /// This allows you to provide your own [`MeterProvider`] implementation for collecting metrics.
    /// By default, the client uses a global meter provider. Use this method to override it for this client instance.
    ///
    /// # Arguments
    ///
    /// * `meter_provider` - A reference to a custom [`MeterProvider`] to use for metrics collection.
    ///
    /// # Performance
    ///
    /// For thread-isolated runtimes, it's preferable to use per-thread instance of meter provider
    /// to avoid lock contention that happens when using a global meter provider.
    ///
    /// [`MeterProvider`]: https://docs.rs/opentelemetry/latest/opentelemetry/metrics/trait.MeterProvider.html
    #[cfg_attr(test, mutants::skip)] // FIXME: mutants remove resilience context and other fields, which we can't really assert on
    pub fn meter_provider(self, meter_provider: &dyn MeterProvider) -> Self {
        // Update the metering at all relevant places
        Self {
            metering: Metering::custom(meter_provider),
            resilience_context: self.resilience_context.use_metrics(meter_provider),
            ..self
        }
    }

    /// Configures the standard pipeline with custom settings.
    ///
    /// This method allows you to customize the standard pipeline (which includes resilience
    /// features and observability) by providing a configuration function that receives
    /// the current pipeline and returns a modified version.
    ///
    /// See [`StandardRequestPipeline`] for more details on the defaults.
    ///
    /// # Multiple Calls
    ///
    /// Multiple consecutive calls to this method are additive - each call receives the
    /// pipeline configured by the previous call. However, if you switch to a different
    /// pipeline type (e.g., [`minimal_pipeline`](Self::minimal_pipeline)) and then call
    /// this method again, it will receive a fresh default [`StandardRequestPipeline`] rather than
    /// the previously configured one.
    ///
    /// # Example
    ///
    /// ```rust
    /// # use std::time::Duration;
    /// # use fetch::{HttpClient, HttpClientBuilder};
    /// # fn configure_builder(mut builder: HttpClientBuilder) {
    /// let client = builder
    ///     .standard_pipeline(|pipeline, _context| {
    ///         // Change the attempt timeout to 5 seconds
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

    /// Sets the base URI for client.
    ///
    /// This setting overrides any endpoint set in the Uri type you pass to the request methods,
    /// leading to three possible scenarios:
    ///
    /// - `HttpClientBuilder::base_uri` is set - [`BaseUri`] on request's [`Uri`](templated_uri::Uri) is ignored and the client uses the provided [`BaseUri`] instead.
    /// - `HttpClientBuilder::base_uri` is not set, but the request [`Uri`](templated_uri::Uri) has a [`BaseUri`] - the client uses the [`BaseUri`] from the request's [`Uri`](templated_uri::Uri).
    /// -  No endpoint is set on either side - the builder fails with `Validation` [`Error`](crate::HttpError)
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

    /// Configures the [`Router`] used to resolve the destination [`BaseUri`] for each request.
    ///
    /// A router can expose multiple alternative endpoints (e.g. a primary and one or more
    /// fallback endpoints). When the configured router has alternatives, the standard pipeline
    /// automatically enables retry/hedging on connection-unavailable errors so subsequent
    /// attempts can target a different endpoint.
    ///
    /// This setting and [`HttpClientBuilder::base_uri`] are mutually exclusive shortcuts for the
    /// same underlying configuration; whichever is called last wins. Calling `base_uri(uri)` is
    /// equivalent to `router(Router::fixed(uri))`.
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

    /// Configures HTTP/2 options for the client.
    ///
    /// This method allows you to customize HTTP/2-specific settings for connections created
    /// by the client. These settings only apply when the client negotiates HTTP/2 connections.
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

    /// Configures connection pool options for the client.
    ///
    /// This method allows you to configure how the client manages its connection pool.
    /// The connection pool reuses existing connections to reduce the overhead and latency
    /// of establishing new connections for each request.
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

    /// Sets the redaction engine for client.
    ///
    /// The [`RedactionEngine`] is used to redact sensitive information from requests and responses.
    /// This is particularly useful for logging and telemetry, where you want to avoid exposing
    /// sensitive data such as authentication tokens, personal information, or other confidential
    /// information.
    pub fn redaction_engine(mut self, redaction_engine: &RedactionEngine) -> Self {
        self.options.redaction_engine = redaction_engine.clone();
        self
    }

    /// Builds the configured HTTP client.
    ///
    /// This finalizes all configuration settings and creates the actual client
    /// instance. After calling this method, you'll have a fully functional
    /// [`HttpClient`](crate::HttpClient) ready to make HTTP requests.
    ///
    /// # Returns
    ///
    /// A new [`HttpClient`](crate::HttpClient) instance configured according to
    /// the settings specified on this builder.
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
            Isolation::Isolated => HttpClientPipeline::Isolated(thread_aware::Arc::new_with(aware, Aware::into_pipeline)),
            Isolation::Shared => HttpClientPipeline::Shared(std::sync::Arc::new(aware.into_pipeline())),
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
        let dispatch = create_dispatch_handler(&meter, self.options.clone(), &self.transport);
        let body_builder = self.transport.create_body_builder(&self.options);

        self.pipeline.build(
            dispatch,
            self.resilience_context,
            self.options.redaction_engine,
            &meter,
            body_builder,
            self.transport.clock().clone(),
            self.options.router,
        )
    }
}

fn create_dispatch_handler(meter: &Meter, options: ClientOptions, transport: &Transport) -> Dispatch {
    let mode = match options.transport.connection_pool.multiple_pools.clone() {
        Some((pool_count, selection)) if pool_count > 1 => {
            let transports = (0..pool_count)
                .map(|index| transport.create_transport_handler(options.clone(), meter.clone(), PoolIndex::new(index)))
                .collect::<Vec<_>>();

            DispatchMode::pooled(transports, selection)
        }
        _ => DispatchMode::single(transport.create_transport_handler(options.clone(), meter.clone(), PoolIndex::new(0))),
    };

    Dispatch::new(mode, options.transport.request_filter)
}

#[cfg(test)]
mod tests {
    use http::StatusCode;
    use http_extensions::{HttpBodyBuilder, HttpBodyOptions};

    use crate::fake::FakeDeps;
    use crate::options::{ConnectionIdleTimeout, ConnectionPoolOptions, Http2Options};
    use crate::telemetry::Metering;
    use crate::{HttpClient, HttpClientBuilder};

    static_assertions::assert_impl_all!(HttpClientBuilder: Send, Sync, Clone);

    #[test]
    fn standard_pipeline_customization_applied() {
        let client = HttpClient::builder_fake(StatusCode::OK, FakeDeps::default())
            .standard_pipeline(|pipeline, _context| pipeline.retry(|retry| retry.max_retry_attempts(5)))
            .build();

        let dbg = client.pipeline().dbg_string_for_custom_pipeline();
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

    #[tokio::test]
    async fn tls_options_are_stored_without_breaking_the_pipeline() {
        use crate::tls::TlsOptions;

        // The fake transport ignores TLS, so a custom `TlsOptions` must be accepted and
        // stored without affecting request handling.
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

        // The stored engine must be the configured one: a passthrough redactor leaves the
        // path untouched.
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

        let dbg = client.pipeline().dbg_string_for_custom_pipeline();
        assert!(dbg.contains("max_attempts: 4"));
    }

    #[tokio::test]
    async fn response_body_options() {
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

        // Verify HTTP/2 options
        assert_eq!(builder.options.transport.http_2.initial_max_send_streams, Some(200));

        // Verify connection pool options
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
        // Verify that the builder can be created with default settings
        let _builder = HttpClient::builder_fake(StatusCode::OK, FakeDeps::default());
    }

    #[test]
    fn name_ok() {
        // Verify that the name method can be called without panic
        let _builder = HttpClient::builder_fake(StatusCode::OK, FakeDeps::default()).name("custom_client");
    }

    #[test]
    fn test_clone_produces_isolated_instances() {
        use std::time::Duration;

        // Create a base builder with some initial configuration
        let builder_1 = HttpClient::builder_fake(StatusCode::OK, FakeDeps::default())
            .name("base_client")
            .connect_timeout(Duration::from_secs(10))
            .response_body_options(HttpBodyOptions::default().buffer_limit(1000));

        // Clone the builder
        let mut builder_2 = builder_1.clone();

        // Verify initial state is the same
        assert_eq!(
            builder_1.options.transport.connect_timeout,
            builder_2.options.transport.connect_timeout
        );
        assert_eq!(builder_1.options.response_body_options, builder_2.options.response_body_options);

        // Modify the cloned builder
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

        // Verify that builder_1 remains unchanged
        assert_eq!(builder_1.options.transport.connect_timeout, Duration::from_secs(10));
        assert_eq!(
            builder_1.options.response_body_options,
            HttpBodyOptions::default().buffer_limit(1000)
        );
        assert_eq!(builder_1.options.transport.http_2.initial_max_send_streams, None);
        assert_eq!(builder_1.options.transport.connection_pool.max_connections, usize::MAX);

        // Verify that builder_2 has the new values
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
        assert!(matches!(builder.metering, Metering::Global));

        let builder = builder.meter_provider(&provider);
        assert!(matches!(builder.metering, Metering::Custom(_)));
    }
}
