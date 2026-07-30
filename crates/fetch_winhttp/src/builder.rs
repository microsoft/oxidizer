// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use bytesbuf::mem::GlobalPool;
use fetch::custom::{CustomContext, CustomDeps, Isolation, create_builder};
use fetch::{HttpClient, HttpClientBuilder};
use observed::Sink;
use thread_aware::ThreadAware;
use tick::Clock;

use crate::bindings::Facade;
use crate::transport::{TransportInputs, WinHttpTransport};
use crate::{WinHttpOptions, WinHttpTlsConfig};

/// Dependencies and configuration for the `WinHTTP` transport.
///
/// Construct this type with [`WinHttpDeps::builder`]. The clock, global memory
/// pool, and telemetry sink are mandatory environment dependencies. TLS and
/// WinHTTP-specific transport options use their defaults when omitted.
#[derive(Clone, Debug, ThreadAware)]
#[non_exhaustive]
pub struct WinHttpDeps {
    clock: Clock,
    global_pool: GlobalPool,
    sink: Sink,
    tls: WinHttpTlsConfig,
    options: WinHttpOptions,
}

impl WinHttpDeps {
    /// Starts building dependencies for the `WinHTTP` transport.
    #[must_use]
    pub fn builder() -> WinHttpDepsBuilder {
        WinHttpDepsBuilder {
            clock: None,
            global_pool: None,
            sink: None,
            tls: WinHttpTlsConfig::default(),
            options: WinHttpOptions::default(),
        }
    }

    pub(crate) fn global_pool(&self) -> &GlobalPool {
        &self.global_pool
    }

    pub(crate) fn sink(&self) -> &Sink {
        &self.sink
    }

    pub(crate) fn tls(&self) -> &WinHttpTlsConfig {
        &self.tls
    }

    pub(crate) fn options(&self) -> &WinHttpOptions {
        &self.options
    }
}

/// Builds [`WinHttpDeps`].
#[derive(Clone, Debug, ThreadAware)]
#[non_exhaustive]
pub struct WinHttpDepsBuilder {
    clock: Option<Clock>,
    global_pool: Option<GlobalPool>,
    sink: Option<Sink>,
    tls: WinHttpTlsConfig,
    options: WinHttpOptions,
}

impl WinHttpDepsBuilder {
    /// Sets the clock used for transport deadlines.
    #[must_use]
    pub fn clock(mut self, clock: Clock) -> Self {
        self.clock = Some(clock);
        self
    }

    /// Sets the global memory pool used for HTTP body and I/O buffers.
    #[must_use]
    pub fn global_pool(mut self, global_pool: GlobalPool) -> Self {
        self.global_pool = Some(global_pool);
        self
    }

    /// Sets the telemetry sink used by the transport.
    #[must_use]
    pub fn sink(mut self, sink: Sink) -> Self {
        self.sink = Some(sink);
        self
    }

    /// Sets the WinHTTP-specific TLS configuration.
    #[must_use]
    pub fn tls(mut self, tls: WinHttpTlsConfig) -> Self {
        self.tls = tls;
        self
    }

    /// Sets the WinHTTP-specific transport options.
    #[must_use]
    pub fn options(mut self, options: WinHttpOptions) -> Self {
        self.options = options;
        self
    }

    /// Builds the `WinHTTP` transport dependencies.
    ///
    /// # Panics
    ///
    /// Panics when `clock`, `global_pool`, or `sink` has not been supplied by
    /// its corresponding setter.
    #[must_use]
    pub fn build(self) -> WinHttpDeps {
        WinHttpDeps {
            clock: self
                .clock
                .expect("WinHttpDeps::build() requires a caller-supplied tick::Clock; call .clock(...) before .build()"),
            global_pool: self.global_pool.expect(
                "WinHttpDeps::build() requires a caller-supplied bytesbuf::mem::GlobalPool; call .global_pool(...) before .build()",
            ),
            sink: self
                .sink
                .expect("WinHttpDeps::build() requires a caller-supplied observed::Sink; call .sink(...) before .build()"),
            tls: self.tls,
            options: self.options,
        }
    }
}

/// Adds `WinHTTP` construction to [`fetch::HttpClient`].
pub trait HttpClientWinHttpExt {
    /// Creates a WinHTTP-backed HTTP client builder.
    ///
    /// Independently built clients use isolated transport resources. Clones of
    /// a built client share that client's transport resources.
    fn builder_winhttp(deps: impl Into<WinHttpDeps>) -> HttpClientBuilder;
}

impl HttpClientWinHttpExt for HttpClient {
    fn builder_winhttp(deps: impl Into<WinHttpDeps>) -> HttpClientBuilder {
        create_builder_with_bindings(deps.into(), Facade::real())
    }
}

fn create_builder_with_bindings(deps: WinHttpDeps, bindings: Facade) -> HttpClientBuilder {
    create_builder(
        "winhttp",
        "winhttp",
        move |context| create_handler(context, bindings.clone()),
        Isolation::Isolated,
        into_custom_deps(deps),
    )
}

fn create_handler(context: CustomContext<WinHttpDeps>, bindings: Facade) -> WinHttpTransport {
    let inputs = TransportInputs {
        body_builder: context.body_builder,
        clock: context.clock,
        global_pool: context.extras.global_pool().clone(),
        sink: context.extras.sink().clone(),
        options: context.options,
        tls: context.extras.tls().clone(),
        session_options: context.extras.options().clone(),
    };
    let _ = (context.pool_index, context.tls, context.meter);

    WinHttpTransport::new(inputs, bindings)
}

fn into_custom_deps(deps: WinHttpDeps) -> CustomDeps<WinHttpDeps> {
    CustomDeps {
        clock: deps.clock.clone(),
        global_pool: deps.global_pool.clone(),
        extras: deps,
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::c_void;
    use std::fmt::Debug;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use bytesbuf::mem::GlobalPool;
    use fetch::options::{ConnectionPoolOptions, PoolSelection};
    use fetch::{Recovery, RecoveryInfo};
    use observed::Sink;
    use static_assertions::{assert_impl_all, assert_not_impl_any};
    use thread_aware::ThreadAware;
    use thread_aware::affinity::pinned_affinities;
    use tick::{Clock, ClockControl};

    use super::{WinHttpDeps, WinHttpDepsBuilder, create_builder_with_bindings, into_custom_deps};
    use crate::bindings::{Facade, MockBindings};
    use crate::handle::RawHandle;
    use crate::{WinHttpOptions, WinHttpTlsConfig};

    assert_impl_all!(WinHttpDeps: Send, Sync, Clone, Debug, ThreadAware);
    assert_impl_all!(WinHttpDepsBuilder: Send, Sync, Clone, Debug, ThreadAware);
    assert_not_impl_any!(WinHttpDeps: Default);
    assert_not_impl_any!(WinHttpDepsBuilder: Default);

    #[test]
    fn optional_configuration_uses_defaults() {
        let deps = complete_builder().build();

        assert_eq!(deps.options().resolve_timeout(), None);
        assert!(!deps.tls().accepts_invalid_certs());
        assert!(!deps.tls().accepts_invalid_hostnames());
    }

    #[test]
    fn optional_configuration_is_forwarded() {
        let timeout = Duration::from_secs(10);
        let deps = complete_builder()
            .tls(WinHttpTlsConfig::builder().accept_invalid_certs(true).build())
            .options(WinHttpOptions::builder().resolve_timeout(timeout).build())
            .build();
        let custom_deps = into_custom_deps(deps);

        assert_eq!(custom_deps.extras.sink().id(), Sink::noop().id());
        assert_eq!(custom_deps.extras.options().resolve_timeout(), Some(timeout));
        assert!(custom_deps.extras.tls().accepts_invalid_certs());

        let _body_pool_buffer = custom_deps.global_pool.reserve(1);
        let _transport_pool_buffer = custom_deps.extras.global_pool().reserve(1);
    }

    #[test]
    fn cloned_client_reuses_one_materialized_session() {
        let (facade, opens, closes) = successful_facade(1);
        let client = create_builder_with_bindings(complete_builder().build(), facade)
            .insecure_allow_http()
            .minimal_pipeline()
            .supported_http_versions(&[http::Version::HTTP_10])
            .build();
        let clone = client.clone();

        assert_eq!(opens.load(Ordering::SeqCst), 1);
        drop(clone);
        assert_eq!(closes.load(Ordering::SeqCst), 0);

        let error = futures::executor::block_on(client.get("http://example.com").fetch()).expect_err("legacy HTTP is rejected");
        assert_eq!(error.recovery(), RecoveryInfo::never());

        drop(client);
        assert_eq!(closes.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn separate_builds_materialize_independent_sessions() {
        let (facade, opens, closes) = successful_facade(2);
        let builder = create_builder_with_bindings(complete_builder().build(), facade)
            .insecure_allow_http()
            .minimal_pipeline()
            .supported_http_versions(&[http::Version::HTTP_10]);
        let first = builder.clone().build();
        let second = builder.build();

        for client in [&first, &second] {
            futures::executor::block_on(client.get("http://example.com").fetch()).expect_err("legacy HTTP is rejected");
        }

        assert_eq!(opens.load(Ordering::SeqCst), 2);
        drop(first);
        drop(second);
        assert_eq!(closes.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn multiple_pool_slots_materialize_independent_sessions() {
        let (facade, opens, closes) = successful_facade(2);
        let client = create_builder_with_bindings(complete_builder().build(), facade)
            .insecure_allow_http()
            .minimal_pipeline()
            .connection_pool_options(
                ConnectionPoolOptions::default().multiple_pools(2, PoolSelection::saturating(PoolSelection::DEFAULT_REQUESTS_PER_CLIENT)),
            )
            .build();

        assert_eq!(opens.load(Ordering::SeqCst), 2);
        drop(client);
        assert_eq!(closes.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn relocation_materializes_an_independent_session_for_the_destination_core() {
        let (facade, opens, closes) = successful_facade(2);
        let client = create_builder_with_bindings(complete_builder().build(), facade)
            .insecure_allow_http()
            .minimal_pipeline()
            .supported_http_versions(&[http::Version::HTTP_10])
            .build();
        let affinities = pinned_affinities(&[2]);
        let mut relocated = client.clone();

        relocated.relocate(None, affinities[1]);

        assert_eq!(opens.load(Ordering::SeqCst), 2);
        futures::executor::block_on(relocated.get("http://example.com").fetch()).expect_err("legacy HTTP is rejected");

        drop(relocated);
        drop(client);
        assert_eq!(closes.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn finite_connection_limit_is_ignored_without_extra_session_options() {
        let (facade, opens, closes) = successful_facade(1);
        let client = create_builder_with_bindings(complete_builder().build(), facade)
            .insecure_allow_http()
            .minimal_pipeline()
            .connection_pool_options(ConnectionPoolOptions::default().max_connections(1))
            .build();

        assert_eq!(opens.load(Ordering::SeqCst), 1);
        drop(client);
        assert_eq!(closes.load(Ordering::SeqCst), 1);
    }

    #[test]
    #[should_panic(expected = "WinHttpDeps::build() requires a caller-supplied tick::Clock; call .clock(...) before .build()")]
    fn missing_clock_has_actionable_panic() {
        let _ = WinHttpDeps::builder().global_pool(GlobalPool::new()).sink(Sink::noop()).build();
    }

    #[test]
    #[should_panic(
        expected = "WinHttpDeps::build() requires a caller-supplied bytesbuf::mem::GlobalPool; call .global_pool(...) before .build()"
    )]
    fn missing_global_pool_has_actionable_panic() {
        let _ = WinHttpDeps::builder().clock(clock()).sink(Sink::noop()).build();
    }

    #[test]
    #[should_panic(expected = "WinHttpDeps::build() requires a caller-supplied observed::Sink; call .sink(...) before .build()")]
    fn missing_sink_has_actionable_panic() {
        let _ = WinHttpDeps::builder().clock(clock()).global_pool(GlobalPool::new()).build();
    }

    fn complete_builder() -> WinHttpDepsBuilder {
        WinHttpDeps::builder()
            .clock(clock())
            .global_pool(GlobalPool::new())
            .sink(Sink::noop())
    }

    fn clock() -> Clock {
        ClockControl::new().to_clock()
    }

    fn successful_facade(session_count: usize) -> (Facade, Arc<AtomicUsize>, Arc<AtomicUsize>) {
        let opens = Arc::new(AtomicUsize::new(0));
        let closes = Arc::new(AtomicUsize::new(0));
        let mut bindings = MockBindings::new();

        let open_count = Arc::clone(&opens);
        bindings.expect_open().times(session_count).returning(move |_, _| {
            let value = open_count.fetch_add(1, Ordering::SeqCst) + 1;
            Ok(raw_handle(value))
        });
        bindings
            .expect_set_timeouts()
            .times(session_count)
            .returning(|_, _, _, _, _| Ok(()));
        bindings.expect_set_option().times(session_count * 2).returning(|_, _, _| Ok(()));
        bindings
            .expect_set_status_callback()
            .times(session_count)
            .returning(|_, _, _| Ok(()));

        let close_count = Arc::clone(&closes);
        bindings.expect_close_handle().times(session_count).returning(move |_| {
            close_count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        });

        (Facade::mock(Arc::new(bindings)), opens, closes)
    }

    fn raw_handle(value: usize) -> RawHandle {
        RawHandle::new(std::ptr::without_provenance_mut::<c_void>(value)).expect("test handle values are nonzero")
    }
}
