// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Arty runtime integration for [`HttpClient`].
//!
//! Create [`ArtyDeps`] from one [`Builtins`] value and a memory pool. This keeps
//! the scheduler, clock, and I/O route on the same Arty worker.
//!
//! Enable the `arty` feature and one TLS feature. Use `rustls` for the default
//! TLS backend or `native-tls` for the platform TLS backend.

use anyspawn::Spawner;
use arty::io::Io;
use arty::task::{Builtins, Scheduler};
use arty_mio::{MioContext, MioOptions};
use bytesbuf::mem::{GlobalPool, OpaqueMemory};
use fetch_hyper::HyperTransportBuilder;
use fetch_mio::{MioConnector, MioConnectorOptions};
use thread_aware::ThreadAware;
use tick::Clock;

use crate::custom::{CustomContext, CustomDeps, Isolation};
use crate::handlers::TransportHandler;
use crate::{HttpClient, HttpClientBuilder};

/// Dependencies for an Arty HTTP client.
///
/// Use [`from_builtins`](Self::from_builtins) to extract the runtime services
/// from one Arty worker. You can also construct this type from explicit
/// services when a dependency container already provides them.
#[derive(Debug, Clone, ThreadAware)]
pub struct ArtyDeps {
    /// Clock for request deadlines and connection timeouts.
    pub clock: Clock,
    /// Scheduler for transport tasks and blocking DNS work.
    pub scheduler: Scheduler,
    /// Thread-aware route to the worker-local Mio context.
    pub io: Io,
    /// Memory pool for HTTP bodies and internal allocations.
    pub global_pool: GlobalPool,
}

impl ArtyDeps {
    /// Extracts dependencies from one Arty worker and uses the supplied memory
    /// pool.
    #[must_use]
    pub fn from_builtins(builtins: &Builtins, global_pool: GlobalPool) -> Self {
        Self {
            clock: builtins.clock().clone(),
            scheduler: builtins.scheduler().clone(),
            io: builtins.io().clone(),
            global_pool,
        }
    }
}

impl From<&Builtins> for ArtyDeps {
    fn from(builtins: &Builtins) -> Self {
        Self::from_builtins(builtins, GlobalPool::new())
    }
}

impl HttpClient {
    /// Creates an HTTP client builder for the Arty runtime.
    ///
    /// The client creates one Hyper transport for each Arty worker. The
    /// connector uses Mio for socket I/O. Its context registers each socket
    /// directly on the calling worker.
    ///
    /// Enable the `arty` feature and one TLS feature to use this method.
    pub fn builder_arty(deps: impl Into<ArtyDeps>) -> HttpClientBuilder {
        let ArtyDeps {
            clock,
            scheduler,
            io,
            global_pool,
        } = deps.into();

        Self::builder_custom_internal(
            crate::constants::ARTY_RUNTIME_NAME,
            crate::constants::HYPER_TRANSPORT_NAME,
            |cx| TransportHandler(build_arty_handler(cx).into()),
            Isolation::Isolated,
            CustomDeps {
                clock,
                memory: OpaqueMemory::new(global_pool),
                extras: ArtyExtras { scheduler, io },
            },
        )
    }

    /// Creates an HTTP client for the Arty runtime.
    ///
    /// Use [`builder_arty`](Self::builder_arty) when the client needs custom
    /// Fetch settings.
    #[must_use]
    pub fn new_arty(deps: impl Into<ArtyDeps>) -> Self {
        Self::builder_arty(deps).build()
    }
}

#[derive(Clone, Debug, ThreadAware)]
struct ArtyExtras {
    scheduler: Scheduler,
    io: Io,
}

fn build_arty_handler(cx: CustomContext<ArtyExtras>) -> fetch_hyper::HyperTransport {
    let scheduler = cx.extras.scheduler.clone();
    let context = cx.extras.io.context::<MioContext>(&MioOptions::default());
    let connector = MioConnector::new(context, scheduler.clone(), cx.clock.clone(), MioConnectorOptions::default());
    let tls_backend = crate::tls::build_tls_backend(&cx.options, cx.tls);

    HyperTransportBuilder::new(connector, Spawner::new_arty(scheduler), cx.clock, cx.options)
        .body_builder(cx.body_builder)
        .pool_index(cx.pool_index)
        .meter(cx.meter)
        .build(tls_backend)
}
