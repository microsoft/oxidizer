// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::io;
use std::net::{IpAddr, SocketAddr, TcpListener};
use std::thread::{self, JoinHandle};

use tokio::runtime::{Builder, Runtime};
use tokio::sync::oneshot;

pub(super) struct Startup<S, W> {
    pub(super) address: SocketAddr,
    pub(super) state: S,
    pub(super) shutdown: Option<oneshot::Sender<()>>,
    pub(super) thread: Option<W>,
}

pub(super) trait StartupListener {
    fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()>;
    fn local_addr(&self) -> io::Result<SocketAddr>;
}

impl StartupListener for TcpListener {
    // #[gamma::skip(fn_value.ok, parameter.default_shadow, tag = "native-adapter", reason = "f7f37c3eb7e7/c7171e97eddb: concrete OS nonblocking adapter; observing this call or its sole bool argument requires a live TCP socket. Not equivalent; generic listener policy remains eligible for mutation")]
    fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        Self::set_nonblocking(self, nonblocking)
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        Self::local_addr(self)
    }
}

pub(super) fn bind_listener<L: StartupListener>(bind: IpAddr, bind_listener: impl FnOnce(SocketAddr) -> L) -> (L, SocketAddr) {
    let listener = bind_listener((bind, 0).into());
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    (listener, address)
}

pub(super) trait WorkerRuntime {
    fn block_on(&self, work: impl Future<Output = ()>);
}

impl WorkerRuntime for Runtime {
    // #[gamma::skip(fn_value.unit, tag = "native-adapter", reason = "b570589bb3db: this concrete Tokio adapter only delegates block_on; executing it requires a real Runtime. Not equivalent; generic run_worker and fake runtime remain eligible for mutation")]
    fn block_on(&self, work: impl Future<Output = ()>) {
        // #[gamma::skip(stmt.delete_call, tag = "native-adapter", reason = "4dd341eb3733: the sole concrete Runtime delegation requires a real runtime to observe. Not equivalent; no generic worker behavior is excluded")]
        Self::block_on(self, work);
    }
}

pub(super) fn run_worker(runtime: &impl WorkerRuntime, work: impl Future<Output = ()>) {
    runtime.block_on(work);
}

// #[gamma::skip(parameter.default_shadow, tag = "native-adapter", reason = "dc591ca11472: name is the only concrete defaultable argument; observing native thread naming requires spawning its real worker/runtime. Not equivalent; spawn, runtime construction, and worker calls retain their other mutators")]
pub(super) fn launch_worker(
    name: String,
    make_runtime: impl FnOnce() -> Builder + Send + 'static,
    work: impl Future<Output = ()> + Send + 'static,
) -> JoinHandle<()> {
    thread::Builder::new()
        .name(name)
        .spawn(move || {
            let runtime = make_runtime().enable_all().build().unwrap();
            // #[gamma::skip(call.replace_with_default, stmt.delete_call, tag = "native-adapter", reason = "7b192f43c15d/45bbc05672d9: dispatch inside the concrete spawned worker requires a native thread/runtime. Not equivalent; only this dispatch occurrence is excluded, not generic run_worker or runtime construction")]
            run_worker(&runtime, work);
        })
        .unwrap()
}

pub(super) trait WorkerLauncher {
    type Worker;

    fn launch(self, name: String, work: impl Future<Output = ()> + Send + 'static) -> Self::Worker;
}

pub(super) struct ThreadLauncher;

impl WorkerLauncher for ThreadLauncher {
    type Worker = JoinHandle<()>;

    // #[gamma::skip(parameter.default_shadow, tag = "native-adapter", reason = "7ebbe3af8923: name is the only concrete defaultable argument at this native launcher boundary; observing it requires a spawned thread/runtime. Not equivalent; generic launcher forwarding remains eligible for mutation")]
    fn launch(self, name: String, work: impl Future<Output = ()> + Send + 'static) -> Self::Worker {
        launch_worker(name, Builder::new_multi_thread, work)
    }
}

#[cfg(test)]
pub(super) mod gamma_round2 {
    use std::cell::Cell;
    use std::sync::Arc;

    use super::*;

    pub(in super::super) struct GammaListener {
        pub(in super::super) address: SocketAddr,
        pub(in super::super) nonblocking: Cell<Option<bool>>,
    }

    impl StartupListener for GammaListener {
        fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
            assert_eq!(self.nonblocking.replace(Some(nonblocking)), None);
            Ok(())
        }

        fn local_addr(&self) -> io::Result<SocketAddr> {
            assert_eq!(self.nonblocking.get(), Some(true));
            Ok(self.address)
        }
    }

    pub(in super::super) struct GammaRuntime;

    impl WorkerRuntime for GammaRuntime {
        fn block_on(&self, work: impl Future<Output = ()>) {
            futures::executor::block_on(work);
        }
    }

    pub(in super::super) struct GammaLauncher {
        pub(in super::super) name: &'static str,
        pub(in super::super) worker: u8,
    }

    impl WorkerLauncher for GammaLauncher {
        type Worker = u8;

        fn launch(self, name: String, work: impl Future<Output = ()> + Send + 'static) -> Self::Worker {
            assert_eq!(name, self.name);
            run_worker(&GammaRuntime, work);
            self.worker
        }
    }

    pub(in super::super) fn tls_config() -> rustls::ServerConfig {
        rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_cert_resolver(Arc::new(rustls::server::ResolvesServerCertUsingSni::new()))
    }
}
