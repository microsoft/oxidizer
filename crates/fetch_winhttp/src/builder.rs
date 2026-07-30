// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use bytesbuf::mem::GlobalPool;
use fetch::custom::{CustomContext, CustomDeps, Isolation, create_builder};
use fetch::{HttpClient, HttpClientBuilder, HttpError, HttpRequest, HttpResponse, RecoveryInfo};
use layered::Service;
use observed::Sink;
use thread_aware::ThreadAware;
use tick::Clock;

use crate::{WinHttpOptions, WinHttpTlsConfig};

const UNAVAILABLE_MESSAGE: &str = "WinHTTP request handling is unavailable";
const ERROR_LABEL: &str = "winhttp_initialization";

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

    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "configuration access is part of the transport module boundary")
    )]
    pub(crate) fn global_pool(&self) -> &GlobalPool {
        &self.global_pool
    }

    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "configuration access is part of the transport module boundary")
    )]
    pub(crate) fn sink(&self) -> &Sink {
        &self.sink
    }

    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "configuration access is part of the transport module boundary")
    )]
    pub(crate) fn tls(&self) -> &WinHttpTlsConfig {
        &self.tls
    }

    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "configuration access is part of the transport module boundary")
    )]
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
        create_builder(
            "winhttp",
            "winhttp",
            create_handler,
            Isolation::Isolated,
            into_custom_deps(deps.into()),
        )
    }
}

fn into_custom_deps(deps: WinHttpDeps) -> CustomDeps<WinHttpDeps> {
    CustomDeps {
        clock: deps.clock.clone(),
        global_pool: deps.global_pool.clone(),
        extras: deps,
    }
}

fn create_handler(context: CustomContext<WinHttpDeps>) -> PermanentFailureHandler {
    PermanentFailureHandler { deps: context.extras }
}

#[derive(Debug)]
struct PermanentFailureHandler {
    deps: WinHttpDeps,
}

impl Service<HttpRequest> for PermanentFailureHandler {
    type Out = fetch::Result<HttpResponse>;

    async fn execute(&self, input: HttpRequest) -> Self::Out {
        let _ = &self.deps;

        Err(HttpError::other(UNAVAILABLE_MESSAGE, RecoveryInfo::never(), ERROR_LABEL).with_request(input))
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Debug;
    use std::time::Duration;

    use bytesbuf::mem::GlobalPool;
    use fetch::{HttpClient, Recovery, RecoveryInfo};
    use observed::Sink;
    use static_assertions::{assert_impl_all, assert_not_impl_any};
    use thread_aware::ThreadAware;
    use tick::{Clock, ClockControl};

    use super::{HttpClientWinHttpExt, PermanentFailureHandler, UNAVAILABLE_MESSAGE, WinHttpDeps, WinHttpDepsBuilder, into_custom_deps};
    use crate::{WinHttpOptions, WinHttpTlsConfig};

    assert_impl_all!(WinHttpDeps: Send, Sync, Clone, Debug, ThreadAware);
    assert_impl_all!(WinHttpDepsBuilder: Send, Sync, Clone, Debug, ThreadAware);
    assert_impl_all!(PermanentFailureHandler: Send, Sync, Debug);
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
    fn factory_creates_permanent_failure_handler_without_os_setup() {
        let client = HttpClient::builder_winhttp(complete_builder().build())
            .insecure_allow_http()
            .minimal_pipeline()
            .build();

        let error = futures::executor::block_on(client.get("http://example.com").fetch())
            .expect_err("the request-handler shell must fail every request");

        assert!(error.to_string().contains(UNAVAILABLE_MESSAGE));
        assert_eq!(error.recovery(), RecoveryInfo::never());
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
}
