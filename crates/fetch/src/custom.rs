// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Custom transport-handler entry points for [`HttpClient`].
//!
//! Every [`HttpClient`] ultimately dispatches requests through a *transport
//! handler* — the leaf of the request pipeline that actually performs I/O.
//! This module exposes the types needed to supply your own, while the bundled
//! transports (the Tokio transport and the test fakes) reuse the same machinery
//! internally.
//!
//! The free-standing [`create_builder`] function is the entry point: it returns
//! an [`HttpClientBuilder`] so the pipeline (middleware, options, …) can be
//! tailored before [`HttpClientBuilder::build`] is called.

use std::fmt::Debug;
use std::sync::Arc;

use bytesbuf::mem::GlobalPool;
use http_extensions::{HttpBodyBuilder, RequestHandler};
use opentelemetry::metrics::Meter;
use thread_aware::{PerCore, ThreadAware, unaware};
use tick::Clock;

use crate::handlers::TransportHandler;
use crate::options::{ClientOptions, PoolIndex, TransportOptions};
use crate::tls::TlsOptions;
use crate::{HttpClient, HttpClientBuilder};

/// Threading model required by a custom transport.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ThreadAware)]
pub enum Isolation {
    /// Each core owns its own pipeline; the factory is invoked once per core.
    Isolated,
    /// A single pipeline is shared across all cores.
    Shared,
}

/// Runtime-agnostic dependencies required by a custom-transport [`HttpClient`].
///
/// The caller is responsible for supplying a suitable [`Clock`] (e.g. `Clock::new_tokio()`
/// or a controlled clock for tests), because this type does not assume any specific runtime.
///
/// The `Extras` type parameter (defaulting to `()`) lets the caller thread additional
/// thread-aware dependencies through to [`CustomContext::extras`] — for example a
/// connection pool, credential provider, or runtime handle.
#[derive(Debug, Clone, ThreadAware)]
pub struct CustomDeps<Extras = ()>
where
    Extras: ThreadAware + Send + Sync + Clone + 'static,
{
    /// Clock for timing operations and timeouts.
    pub clock: Clock,
    /// Memory pool for usage-neutral memory allocations.
    pub global_pool: GlobalPool,
    /// Extra dependencies forwarded verbatim to [`CustomContext::extras`].
    pub extras: Extras,
}

/// Per-pool-slot context handed to a user-supplied transport factory.
///
/// The client constructs one [`CustomContext`] each time it needs a new transport handler
/// (typically once per connection pool slot, per core). `Extras` mirrors the same
/// parameter on [`CustomDeps`].
#[derive(Debug)]
#[non_exhaustive]
pub struct CustomContext<Extras = ()> {
    /// Builder for assembling HTTP response bodies, backed by the client's memory pool.
    pub body_builder: HttpBodyBuilder,
    /// Clock for timing operations and timeouts inside the handler.
    pub clock: Clock,
    /// Index of the connection pool slot this handler will service.
    pub pool_index: PoolIndex,
    /// Caller-supplied extras, cloned from [`CustomDeps::extras`].
    pub extras: Extras,

    /// Transport-level options configured on the client.
    ///
    /// Custom transports can honor these knobs (connect timeout, keep-alive,
    /// supported HTTP versions, connection-pool sizing, ...) when establishing
    /// connections.
    pub options: TransportOptions,

    /// TLS configuration declared on the client.
    ///
    /// A custom transport that terminates `https://` connections itself can read
    /// this to honor the caller's certificate, `ALPN`, and backend preferences.
    /// The bundled Tokio transport consumes it to build its TLS connector; custom
    /// transports that only serve `http://` may ignore it.
    pub tls: TlsOptions,

    /// Telemetry meter the client records its metrics against.
    ///
    /// A custom transport can use this same [`Meter`] to emit its own instruments
    /// so that transport-level metrics share the client's meter scope.
    pub meter: Meter,
}

/// Creates a builder for an HTTP client backed by a custom transport handler.
///
/// `factory` is invoked lazily, once per pool slot, with a [`CustomContext`] for that
/// slot, and must return a [`RequestHandler`] that becomes the transport stage of the
/// pipeline. The full request pipeline (resilience, telemetry, logging, ...)
/// is layered on top by the builder.
///
/// `isolation` selects the threading model the underlying transport requires; see
/// [`Isolation`].
///
/// The `Extras` parameter on [`CustomDeps`] / [`CustomContext`] plumbs additional
/// thread-aware dependencies through to `factory` without resorting to globals.
/// Leave it defaulted to `()` when no extras are needed.
///
/// Because the handler is the transport stage, the caller is responsible for TLS:
/// if `https://` URIs are expected, the handler must negotiate TLS itself.
/// Otherwise pair the builder with [`HttpClientBuilder::insecure_allow_http`] and
/// only issue `http://` requests.
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
pub fn create_builder<F, R, Extras>(factory: F, isolation: Isolation, deps: impl Into<CustomDeps<Extras>>) -> HttpClientBuilder
where
    F: Fn(CustomContext<Extras>) -> R + Send + Sync + 'static,
    R: RequestHandler + 'static,
    Extras: ThreadAware + Send + Sync + Clone + 'static,
{
    // Type-erase the user-supplied handler into `TransportHandler` once, then
    // delegate to the in-crate path shared with the bundled transports.
    HttpClient::builder_custom_internal(move |cx| TransportHandler::new(factory(cx)), isolation, deps.into())
}

impl HttpClient {
    /// In-crate variant of [`create_builder`] used by the bundled
    /// Tokio transport. The factory produces a pre-erased [`TransportHandler`], avoiding a
    /// redundant boxing step for transports that branch over multiple concrete handler
    /// types. The `tls`/`meter` fields on [`CustomContext`] are populated unconditionally
    /// for the bundled transports; user-supplied factories may read or ignore them.
    pub(crate) fn builder_custom_internal<F, Extras>(factory: F, isolation: Isolation, deps: CustomDeps<Extras>) -> HttpClientBuilder
    where
        F: Fn(CustomContext<Extras>) -> TransportHandler + Send + Sync + 'static,
        Extras: ThreadAware + Send + Sync + Clone + 'static,
    {
        // The factory is shared across cores via `Arc`. The original `CustomDeps` is
        // carried alongside it so its `extras` are cloned into a fresh `CustomContext`
        // for every handler the per-core transport builds.
        let factory = Arc::new(factory);

        let transport = Transport {
            clock: deps.clock.clone(),
            global_pool: deps.global_pool.clone(),
            isolation,
            inner: thread_aware::Arc::new_with((deps, unaware(factory)), |(deps, factory)| {
                Arc::new(move |options, meter, pool_index| {
                    let context = CustomContext {
                        body_builder: create_body_builder(&deps.global_pool, &deps.clock, &options),
                        clock: deps.clock.clone(),
                        pool_index,
                        extras: deps.extras.clone(),
                        options: options.transport.clone(),
                        tls: options.tls.clone(),
                        meter,
                    };
                    factory.0(context)
                })
            }),
        };

        HttpClientBuilder::new(transport)
    }
}

type TransportFn = Arc<dyn Fn(ClientOptions, Meter, PoolIndex) -> TransportHandler + Send + Sync>;

#[derive(Clone, ThreadAware)]
pub(crate) struct Transport {
    inner: thread_aware::Arc<TransportFn, PerCore>,
    clock: Clock,
    global_pool: GlobalPool,
    isolation: Isolation,
}

impl Transport {
    pub fn create_transport_handler(&self, options: ClientOptions, meter: Meter, index: PoolIndex) -> TransportHandler {
        self.inner.as_ref()(options, meter, index)
    }

    pub fn clock(&self) -> &Clock {
        &self.clock
    }

    pub fn isolation(&self) -> Isolation {
        self.isolation
    }

    pub fn create_body_builder(&self, options: &ClientOptions) -> HttpBodyBuilder {
        create_body_builder(&self.global_pool, &self.clock, options)
    }
}

impl Debug for Transport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct(std::any::type_name::<Self>()).finish()
    }
}

pub(crate) fn create_body_builder(pool: &GlobalPool, clock: &Clock, options: &ClientOptions) -> HttpBodyBuilder {
    HttpBodyBuilder::new(pool.clone(), clock).with_options(options.response_body_options)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

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
            global_pool: bytesbuf::mem::GlobalPool::new(),
            extras: (),
        }
    }

    #[mutants::skip]
    fn ok_factory(_ctx: CustomContext) -> FakeHandler {
        FakeHandler::from_sync_handler(|_req| HttpResponseBuilder::new_fake().status(StatusCode::OK).build())
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn create_builder_serves_requests_through_custom_pipeline() {
        // `create_builder` exposes the full builder so callers can tweak the pipeline
        // (here: switch to the minimal pipeline) before driving a real request.
        let client = create_builder(ok_factory, Isolation::Shared, custom_deps())
            .insecure_allow_http()
            .minimal_pipeline()
            .build();

        assert!(matches!(client.pipeline(), Pipeline::Minimal(_)));

        let response = client.get("http://example.com").fetch().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn isolated_runtime_uses_per_core_handler() {
        // `Isolation::Isolated` is the right choice for thread-per-core transports;
        // it must still serve requests correctly when there is only one core in play.
        let client = create_builder(ok_factory, Isolation::Isolated, custom_deps())
            .insecure_allow_http()
            .build();

        let response = client.get("http://example.com").fetch().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn extras_are_forwarded_to_factory() {
        // `Arc<AtomicUsize>` is not `ThreadAware`-deriveable, but it is safe to share
        // across threads, so wrap it in `unaware` to use as extras.
        let counter = Arc::new(AtomicUsize::new(0));
        let deps = CustomDeps {
            clock: FakeDeps::default().clock,
            global_pool: bytesbuf::mem::GlobalPool::new(),
            extras: unaware(Arc::clone(&counter)),
        };

        let client = create_builder(
            |ctx: CustomContext<thread_aware::Unaware<Arc<AtomicUsize>>>| {
                // Touching `extras` during factory invocation proves the value travels
                // all the way through the transport plumbing.
                ctx.extras.fetch_add(1, Ordering::Relaxed);
                FakeHandler::from_sync_handler(|_req| HttpResponseBuilder::new_fake().status(StatusCode::OK).build())
            },
            Isolation::Shared,
            deps,
        )
        .insecure_allow_http()
        .build();

        let response = client.get("http://example.com").fetch().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        // The factory must have been called at least once to build the per-slot handler.
        assert!(counter.load(Ordering::Relaxed) >= 1);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn transport_has_type_name_debug_representation() {
        // `Transport` holds non-Debug closures, so its Debug impl falls back to the type
        // name. The builder embeds the transport, so formatting it exercises that impl.
        let builder = create_builder(ok_factory, Isolation::Shared, custom_deps());

        assert!(format!("{builder:?}").contains("Transport"));
    }
}
