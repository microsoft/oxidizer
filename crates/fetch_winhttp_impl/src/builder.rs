// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use bytesbuf::mem::GlobalPool;
use fetch::custom::{CustomContext, CustomDeps, Isolation, create_builder};
use fetch::{HttpClient, HttpClientBuilder};
use observed::Sink;
use thread_aware::ThreadAware;
use tick::Clock;

use crate::WinHttpTlsConfig;
use crate::bindings::BindingsFacade;
use crate::transport::{TransportInputs, WinHttpTransport};

/// Supplies the environment and transport extras needed by WinHTTP.
///
/// The clock, global memory pool, and telemetry sink are mandatory application
/// services that the transport cannot create for itself. TLS and
/// WinHTTP-specific options are transport extras and use strict or unlimited
/// defaults when omitted.
///
/// One set of dependencies configures every transport a client materializes
/// from it. How those transports organize connections is unspecified; what the
/// transport promises is that clients built independently never share
/// connections.
#[derive(Clone, Debug, ThreadAware)]
#[non_exhaustive]
pub struct WinHttpDeps {
    clock: Clock,
    global_pool: GlobalPool,
    sink: Sink,
    tls: WinHttpTlsConfig,
}

impl WinHttpDeps {
    /// Starts building dependencies for the WinHTTP transport.
    #[must_use]
    pub fn builder(clock: Clock, global_pool: GlobalPool, sink: Sink) -> WinHttpDepsBuilder {
        WinHttpDepsBuilder {
            clock,
            global_pool,
            sink,
            tls: WinHttpTlsConfig::default(),
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
}

/// Collects the inputs that become relocatable [`WinHttpDeps`].
///
/// The builder keeps mandatory environment services together with optional
/// WinHTTP configuration. Building it performs no I/O; the transport acquires
/// its operating-system resources only when a client materializes it.
#[derive(Clone, Debug, ThreadAware)]
#[non_exhaustive]
pub struct WinHttpDepsBuilder {
    clock: Clock,
    global_pool: GlobalPool,
    sink: Sink,
    tls: WinHttpTlsConfig,
}

impl WinHttpDepsBuilder {
    /// Sets the WinHTTP-specific TLS configuration.
    #[must_use]
    pub fn tls(mut self, tls: WinHttpTlsConfig) -> Self {
        self.tls = tls;
        self
    }

    /// Builds the WinHTTP transport dependencies.
    #[must_use]
    pub fn build(self) -> WinHttpDeps {
        WinHttpDeps {
            clock: self.clock,
            global_pool: self.global_pool,
            sink: self.sink,
            tls: self.tls,
        }
    }
}

/// Adds WinHTTP-backed construction to [`fetch::HttpClient`].
///
/// This foreign-type extension trait enters the ordinary `fetch` custom
/// transport pipeline, so callers can configure the same client layers before
/// building. WinHTTP owns TLS through Schannel, so generic `fetch`
/// [`TlsOptions`](fetch::tls::TlsOptions) are ignored; use
/// [`WinHttpTlsConfig`] in [`WinHttpDeps`] for supported TLS controls.
///
/// Independently built clients never share connections, so a client built with
/// relaxed TLS validation cannot reuse a connection a strict client
/// established. Cloning a built client shares that client's transport
/// resources, as the generic contract requires.
///
/// The trait is sealed: only [`HttpClient`] implements it.
pub trait HttpClientWinHttpExt: sealed::Sealed {
    /// Creates a WinHTTP-backed HTTP client builder.
    ///
    /// Independently built clients do not share connections.
    fn builder_winhttp(deps: impl Into<WinHttpDeps>) -> HttpClientBuilder;
}

impl HttpClientWinHttpExt for HttpClient {
    fn builder_winhttp(deps: impl Into<WinHttpDeps>) -> HttpClientBuilder {
        create_builder_with_bindings(deps.into(), BindingsFacade::real())
    }
}

pub(crate) mod sealed {
    use fetch::HttpClient;

    #[expect(unnameable_types, reason = "intentional, sealed trait pattern")]
    pub trait Sealed {}
    impl Sealed for HttpClient {}
}

fn create_builder_with_bindings(deps: WinHttpDeps, bindings: BindingsFacade) -> HttpClientBuilder {
    create_builder(
        "winhttp",
        "winhttp",
        move |context| create_handler(context, bindings.clone()),
        Isolation::Isolated,
        into_custom_deps(deps),
    )
}

fn create_handler(context: CustomContext<WinHttpDeps>, bindings: BindingsFacade) -> WinHttpTransport {
    let inputs = TransportInputs {
        body_builder: context.body_builder,
        clock: context.clock,
        global_pool: context.extras.global_pool().clone(),
        sink: context.extras.sink().clone(),
        options: context.options,
        tls: context.extras.tls().clone(),
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
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::ffi::c_void;
    use std::fmt::Debug;
    use std::panic::{RefUnwindSafe, UnwindSafe};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use bytesbuf::mem::GlobalPool;
    use fetch::options::{ConnectionLifetime, ConnectionPoolOptions, Http2Options, PoolSelection};
    use fetch::tls::TlsOptions;
    use fetch::{Recovery, RecoveryInfo};
    use observed::Sink;
    use static_assertions::{assert_impl_all, assert_not_impl_any};
    use thread_aware::ThreadAware;
    use thread_aware::affinity::pinned_affinities;
    use tick::{Clock, ClockControl};

    use super::{WinHttpDeps, WinHttpDepsBuilder, create_builder_with_bindings, into_custom_deps};
    use crate::WinHttpTlsConfig;
    use crate::bindings::{BindingsFacade, MockBindings};
    use crate::handle::RawHandle;
    use crate::mocks::drive;
    use crate::session::SESSION_OPTIONS_WITHOUT_KEEP_ALIVE;

    assert_impl_all!(WinHttpDeps: Send, Sync, Clone, Debug, ThreadAware);
    assert_impl_all!(WinHttpDepsBuilder: Send, Sync, Clone, Debug, ThreadAware);
    // The configured memory pool and telemetry sink contain user-erased state.
    assert_not_impl_any!(WinHttpDeps: UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(WinHttpDepsBuilder: UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(WinHttpDeps: Default);
    assert_not_impl_any!(WinHttpDepsBuilder: Default);

    #[test]
    fn optional_configuration_uses_defaults() {
        let deps = complete_builder().build();

        assert!(!deps.tls().accepts_invalid_certs());
        assert!(!deps.tls().accepts_invalid_hostnames());
    }

    #[test]
    fn optional_configuration_is_forwarded() {
        let deps = complete_builder()
            .tls(WinHttpTlsConfig::builder().accept_invalid_certs(true).build())
            .build();
        let custom_deps = into_custom_deps(deps);

        assert_eq!(custom_deps.extras.sink().id(), Sink::noop().id());
        assert!(custom_deps.extras.tls().accepts_invalid_certs());

        let _body_pool_buffer = custom_deps.global_pool.reserve(1);
        let _transport_pool_buffer = custom_deps.extras.global_pool().reserve(1);
    }

    #[test]
    fn cloned_client_reuses_one_materialized_session() {
        let (facade, opens, closes) = successful_bindings_facade(1);
        let client = create_builder_with_bindings(complete_builder().build(), facade)
            .insecure_allow_http()
            .minimal_pipeline()
            .supported_http_versions(&[http::Version::HTTP_10])
            .build();
        let clone = client.clone();

        assert_eq!(opens.load(Ordering::SeqCst), 1);
        drop(clone);
        assert_eq!(closes.load(Ordering::SeqCst), 0);

        let error = drive(client.get("http://example.com").fetch()).unwrap_err();
        assert_eq!(error.recovery(), RecoveryInfo::never());

        drop(client);
        assert_eq!(closes.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn separate_builds_materialize_independent_sessions() {
        let (facade, opens, closes) = successful_bindings_facade(2);
        let builder = create_builder_with_bindings(complete_builder().build(), facade)
            .insecure_allow_http()
            .minimal_pipeline()
            .supported_http_versions(&[http::Version::HTTP_10]);
        let first = builder.clone().build();
        let second = builder.build();

        for client in [&first, &second] {
            drive(client.get("http://example.com").fetch()).unwrap_err();
        }

        assert_eq!(opens.load(Ordering::SeqCst), 2);
        drop(first);
        drop(second);
        assert_eq!(closes.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn multiple_pool_slots_materialize_independent_sessions() {
        let (facade, opens, closes) = successful_bindings_facade(2);
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
        let (facade, opens, closes) = successful_bindings_facade(2);
        let client = create_builder_with_bindings(complete_builder().build(), facade)
            .insecure_allow_http()
            .minimal_pipeline()
            .supported_http_versions(&[http::Version::HTTP_10])
            .build();
        let affinities = pinned_affinities(&[2]);
        let mut relocated = client.clone();

        relocated.relocate(None, affinities[1]);

        assert_eq!(opens.load(Ordering::SeqCst), 2);
        drive(relocated.get("http://example.com").fetch()).unwrap_err();

        drop(relocated);
        drop(client);
        assert_eq!(closes.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn unsupported_generic_configuration_is_ignored_without_extra_session_options() {
        let (facade, opens, closes) = successful_bindings_facade(1);
        let client = create_builder_with_bindings(complete_builder().build(), facade)
            .insecure_allow_http()
            .minimal_pipeline()
            .connection_pool_options(
                ConnectionPoolOptions::default()
                    .max_connections(1)
                    .connection_lifetime(ConnectionLifetime::fixed(Duration::from_secs(2))),
            )
            .http2_options(Http2Options::default().initial_max_send_streams(1).adaptive_window(true))
            .tls_options(TlsOptions::default())
            .build();

        assert_eq!(opens.load(Ordering::SeqCst), 1);
        drop(client);
        assert_eq!(closes.load(Ordering::SeqCst), 1);
    }

    fn complete_builder() -> WinHttpDepsBuilder {
        WinHttpDeps::builder(clock(), GlobalPool::new(), Sink::noop())
    }

    fn clock() -> Clock {
        ClockControl::new().to_clock()
    }

    fn successful_bindings_facade(session_count: usize) -> (BindingsFacade, Arc<AtomicUsize>, Arc<AtomicUsize>) {
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
        bindings
            .expect_set_option()
            .times(session_count * SESSION_OPTIONS_WITHOUT_KEEP_ALIVE)
            .returning(|_, _, _| Ok(()));
        bindings
            .expect_set_status_callback()
            .times(session_count)
            .returning(|_, _, _| Ok(()));

        let close_count = Arc::clone(&closes);
        bindings.expect_close_handle().times(session_count).returning(move |_| {
            close_count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        });

        (BindingsFacade::mock(Arc::new(bindings)), opens, closes)
    }

    fn raw_handle(value: usize) -> RawHandle {
        RawHandle::new(std::ptr::without_provenance_mut::<c_void>(value)).unwrap()
    }
}
