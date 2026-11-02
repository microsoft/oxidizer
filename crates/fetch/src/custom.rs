// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Custom transport-handler entry points for [`HttpClient`].
//!
//! A transport handler is the final request-pipeline handler. It performs I/O. This module contains
//! the types for a custom transport. The bundled Tokio transport and test fakes use the same types.
//!
//! [`create_builder`][crate::custom::create_builder] returns an [`HttpClientBuilder`]. Configure
//! the pipeline before you call [`HttpClientBuilder::build`].

use std::borrow::Cow;
use std::fmt::Debug;

use bytesbuf::mem::OpaqueMemory;
use http_extensions::{HttpBodyBuilder, RequestHandler};
use opentelemetry::metrics::Meter;
use performables::arc::{Arc, PerThread};
use thread_aware::{ThreadAware, unaware};
use tick::Clock;

use crate::handlers::TransportHandler;
use crate::options::{ClientOptions, PoolIndex, TransportOptions};
use crate::tls::TlsOptions;
use crate::{HttpClient, HttpClientBuilder};

/// Threading model required by a custom transport.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ThreadAware)]
pub enum Isolation {
    /// Each runtime thread gets a pipeline. The factory runs once for each configured pool slot in
    /// that thread partition.
    Isolated,
    /// A single pipeline is shared across all runtime threads.
    Shared,
}

/// Runtime-agnostic dependencies required by a custom-transport [`HttpClient`].
///
/// The caller supplies a [`Clock`], such as `Clock::new_tokio()` or a controlled test clock. This
/// type does not select a runtime.
///
/// `Extras` defaults to `()`. It passes thread-aware dependencies to
/// [`CustomContext::extras`], such as a connection pool, credential provider, or runtime handle.
///
/// Request bodies and transport body builders use `memory`.
/// Cloning dependencies clones the provider.
/// Thread-aware relocation forwards to the provider.
#[derive(Debug, Clone, ThreadAware)]
pub struct CustomDeps<Extras = ()>
where
    Extras: ThreadAware + Send + Sync + Clone + 'static,
{
    /// Clock for timing operations and timeouts.
    pub clock: Clock,
    /// Memory provider for HTTP body allocations.
    pub memory: OpaqueMemory,
    /// Extra dependencies copied to [`CustomContext::extras`].
    pub extras: Extras,
}

/// Per-pool-slot context handed to a user-supplied transport factory.
///
/// The client creates one [`CustomContext`] for each transport handler. It usually creates one for
/// each connection-pool slot on each thread. `Extras` is the same parameter as on [`CustomDeps`].
#[derive(Debug)]
#[non_exhaustive]
pub struct CustomContext<Extras = ()> {
    /// Builds HTTP response bodies with the client memory provider.
    pub body_builder: HttpBodyBuilder,
    /// Clock for timing operations and timeouts inside the handler.
    pub clock: Clock,
    /// Index of the connection-pool slot that this handler serves.
    pub pool_index: PoolIndex,
    /// Caller-supplied extras, cloned from [`CustomDeps::extras`].
    pub extras: Extras,

    /// Transport-level options configured on the client.
    ///
    /// A custom transport can apply the connection timeout, keep-alive, supported HTTP versions,
    /// connection-pool size, and other transport options.
    pub options: TransportOptions,

    /// TLS configuration declared on the client.
    ///
    /// A custom HTTPS transport can use the certificate, `ALPN`, and backend preferences. The
    /// bundled Tokio transport uses them to build its TLS connector. An HTTP-only transport can
    /// ignore them.
    pub tls: TlsOptions,

    /// Telemetry meter the client records its metrics against.
    ///
    /// A custom transport can use this [`Meter`] so its metrics share the client meter scope.
    pub meter: Meter,
}

/// Creates a builder for an HTTP client backed by a custom transport handler.
///
/// `runtime` and `transport` identify the client in telemetry. Every metric uses them as the
/// `fetch.runtime` and `fetch.transport` instrumentation-scope attributes. Use short, stable
/// identifiers, such as `runtime = "tokio"` and `transport = "http3_experimental"`.
///
/// The client calls `factory` when it needs a handler for a pool slot. The factory receives the
/// slot [`CustomContext`] and returns the transport [`RequestHandler`]. The builder adds resilience,
/// telemetry, logging, and other pipeline layers above it.
///
/// `isolation` selects the transport threading model. See [`Isolation`].
///
/// `Extras` passes thread-aware dependencies from [`CustomDeps`] through [`CustomContext`] to
/// `factory`. Use the default `()` when the factory needs no extra dependencies.
///
/// Pass transport-specific options through `Extras`, not [`TransportOptions`]. This prevents a
/// transport from silently ignoring unsupported options.
///
/// The transport handler is responsible for TLS. It must negotiate TLS for `https://` requests.
/// For an HTTP-only transport, call [`HttpClientBuilder::insecure_allow_http`] and send only
/// `http://` requests.
///
/// # Examples
///
/// ```
/// # use fetch::custom::{create_builder, CustomContext, CustomDeps, Isolation};
/// # use fetch::{HttpBodyBuilder, HttpError, HttpRequest, HttpResponse, HttpResponseBuilder};
/// # use http::StatusCode;
/// # use layered::Service;
/// /// Transport handler that ignores the request and returns a canned `200 OK`.
/// struct MyTransportHandler {
///     body_builder: HttpBodyBuilder,
/// }
///
/// impl Service<HttpRequest> for MyTransportHandler {
///     type Out = fetch::Result<HttpResponse>;
///
///     async fn execute(&self, _request: HttpRequest) -> Self::Out {
///         HttpResponseBuilder::new(&self.body_builder)
///             .status(StatusCode::OK)
///             .build()
///     }
/// }
///
/// # async fn example(deps: CustomDeps) -> Result<(), HttpError> {
/// let client = create_builder(
///     "my-runtime",
///     "my-transport",
///     |ctx: CustomContext| MyTransportHandler {
///         body_builder: ctx.body_builder,
///     },
///     Isolation::Shared,
///     deps,
/// )
/// .insecure_allow_http()
/// .build();
///
/// let response = client.get("http://example.com").fetch().await?;
/// assert_eq!(response.status(), StatusCode::OK);
/// # Ok(())
/// # }
/// ```
pub fn create_builder<F, R, Extras>(
    runtime: impl Into<Cow<'static, str>>,
    transport: impl Into<Cow<'static, str>>,
    factory: F,
    isolation: Isolation,
    deps: impl Into<CustomDeps<Extras>>,
) -> HttpClientBuilder
where
    F: Fn(CustomContext<Extras>) -> R + Send + Sync + 'static,
    R: RequestHandler + 'static,
    Extras: ThreadAware + Send + Sync + Clone + 'static,
{
    // Erase the handler type once, then use the path shared with bundled transports.
    HttpClient::builder_custom_internal(
        runtime,
        transport,
        move |cx| TransportHandler::new(factory(cx)),
        isolation,
        deps.into(),
    )
}

impl HttpClient {
    /// In-crate variant of [`create_builder`] for the bundled Tokio transport.
    ///
    /// The factory returns an erased [`TransportHandler`]. This avoids another boxing step when a
    /// transport can return several handler types. [`CustomContext`] always contains `tls` and
    /// `meter`. A factory can use or ignore them.
    pub(crate) fn builder_custom_internal<F, Extras>(
        runtime: impl Into<Cow<'static, str>>,
        transport: impl Into<Cow<'static, str>>,
        factory: F,
        isolation: Isolation,
        deps: CustomDeps<Extras>,
    ) -> HttpClientBuilder
    where
        F: Fn(CustomContext<Extras>) -> TransportHandler + Send + Sync + 'static,
        Extras: ThreadAware + Send + Sync + Clone + 'static,
    {
        // Share the factory across threads. Copy `extras` into each new `CustomContext`.
        let factory = unaware(Arc::new(factory));

        let transport = Transport {
            runtime_name: runtime.into(),
            name: transport.into(),
            clock: deps.clock.clone(),
            memory: deps.memory.clone(),
            isolation,
            inner: Arc::new_with_data((deps, factory), |(deps, factory)| {
                let transport: Box<dyn Fn(ClientOptions, Meter, PoolIndex) -> TransportHandler + Send + Sync> =
                    Box::new(move |options, meter, pool_index| {
                        let context = CustomContext {
                            body_builder: create_body_builder(&deps.memory, &deps.clock, &options),
                            clock: deps.clock.clone(),
                            pool_index,
                            extras: deps.extras.clone(),
                            options: options.transport.clone(),
                            tls: options.tls.clone(),
                            meter,
                        };
                        factory(context)
                    });
                TransportFn::from(transport)
            }),
        };

        HttpClientBuilder::new(transport)
    }
}

type TransportFn = Arc<dyn Fn(ClientOptions, Meter, PoolIndex) -> TransportHandler + Send + Sync>;

#[derive(Clone, ThreadAware)]
pub(crate) struct Transport {
    #[thread_aware(skip)]
    runtime_name: Cow<'static, str>,
    #[thread_aware(skip)]
    name: Cow<'static, str>,
    inner: Arc<TransportFn, PerThread>,
    clock: Clock,
    memory: OpaqueMemory,
    isolation: Isolation,
}

impl Transport {
    pub(crate) fn create_transport_handler(&self, options: ClientOptions, meter: Meter, index: PoolIndex) -> TransportHandler {
        self.inner.as_ref()(options, meter, index)
    }

    pub(crate) fn runtime(&self) -> &Cow<'static, str> {
        &self.runtime_name
    }

    pub(crate) fn name(&self) -> &Cow<'static, str> {
        &self.name
    }

    pub(crate) fn clock(&self) -> &Clock {
        &self.clock
    }

    pub(crate) fn isolation(&self) -> Isolation {
        self.isolation
    }

    pub(crate) fn create_body_builder(&self, options: &ClientOptions) -> HttpBodyBuilder {
        create_body_builder(&self.memory, &self.clock, options)
    }
}

impl Debug for Transport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct(std::any::type_name::<Self>()).finish()
    }
}

pub(crate) fn create_body_builder(memory: &OpaqueMemory, clock: &Clock, options: &ClientOptions) -> HttpBodyBuilder {
    HttpBodyBuilder::new(memory.clone(), clock).with_options(options.response_body_options)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use bytesbuf::mem::{GlobalPool, OpaqueMemory};
    use http::StatusCode;
    use http_extensions::FakeHandler;
    use thread_aware::unaware;

    use super::{CustomContext, CustomDeps, Isolation, create_builder};
    use crate::HttpResponseBuilder;
    use crate::fake::FakeDeps;
    use crate::pipeline::Pipeline;

    #[mutants::skip]
    fn custom_deps() -> CustomDeps {
        CustomDeps {
            clock: FakeDeps::default().clock,
            memory: OpaqueMemory::new(GlobalPool::new()),
            extras: (),
        }
    }

    #[mutants::skip]
    fn ok_factory(_ctx: CustomContext) -> FakeHandler {
        FakeHandler::from_fn(|_req| HttpResponseBuilder::new_fake().status(StatusCode::OK).build())
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn create_builder_serves_requests_through_custom_pipeline() {
        // Select the minimal pipeline through the builder before sending a request.
        let client = create_builder("test-runtime", "test", ok_factory, Isolation::Shared, custom_deps())
            .insecure_allow_http()
            .minimal_pipeline()
            .build();

        assert!(matches!(client.pipeline(), Pipeline::Minimal(_)));

        let response = client.get("http://example.com").fetch().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn isolated_runtime_uses_per_thread_handler() {
        // An isolated transport also serves requests in a single-thread runtime.
        let client = create_builder("test-runtime", "test", ok_factory, Isolation::Isolated, custom_deps())
            .insecure_allow_http()
            .build();

        let response = client.get("http://example.com").fetch().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn extras_are_forwarded_to_factory() {
        // Wrap Arc<AtomicUsize> in unaware because derive cannot implement ThreadAware for it.
        let counter = Arc::new(AtomicUsize::new(0));
        let deps = CustomDeps {
            clock: FakeDeps::default().clock,
            memory: OpaqueMemory::new(GlobalPool::new()),
            extras: unaware(Arc::clone(&counter)),
        };

        let client = create_builder(
            "test-runtime",
            "test",
            |ctx: CustomContext<thread_aware::Unaware<Arc<AtomicUsize>>>| {
                // Reading extras proves that the factory received the value.
                ctx.extras.fetch_add(1, Ordering::Relaxed);
                FakeHandler::from_fn(|_req| HttpResponseBuilder::new_fake().status(StatusCode::OK).build())
            },
            Isolation::Shared,
            deps,
        )
        .insecure_allow_http()
        .build();

        let response = client.get("http://example.com").fetch().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        // Building a handler calls the factory at least once.
        assert!(counter.load(Ordering::Relaxed) >= 1);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn transport_has_type_name_debug_representation() {
        // Transport contains closures without Debug, so its Debug output uses the type name.
        let builder = create_builder("test-runtime", "test", ok_factory, Isolation::Shared, custom_deps());

        assert!(format!("{builder:?}").contains("Transport"));
    }
}
