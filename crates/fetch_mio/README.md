<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Fetch Mio Logo" width="96">

# Fetch Mio

[![crate.io](https://img.shields.io/crates/v/fetch_mio.svg)](https://crates.io/crates/fetch_mio)
[![docs.rs](https://docs.rs/fetch_mio/badge.svg)](https://docs.rs/fetch_mio)
[![MSRV](https://img.shields.io/crates/msrv/fetch_mio)](https://crates.io/crates/fetch_mio)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml/badge.svg)](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Arty Mio connector for [`fetch_hyper`][__link0].

[`MioConnector`][__link1] resolves HTTP endpoints and opens nonblocking Mio TCP
streams through [`arty_mio`][__link2]. [`MioStream`][__link3] implements
[`fetch_hyper::HyperIo`][__link4].

Create one connector on each Arty worker. [`arty_mio::MioContext`][__link5] provides
the thread-aware route for direct source registration. The connector is
cloneable for Hyper, but it does not implement `ThreadAware`.

Use [`anyspawn::Spawner::new_arty`][__link6] with the same worker scheduler when you
build a [`fetch_hyper::HyperTransportBuilder`][__link7].

```rust
use anyspawn::Spawner;
use arty::task::Builtins;
use arty_mio::{MioContext, MioOptions};
use fetch_mio::{MioConnector, MioConnectorOptions};

fn worker_transport_parts(builtins: &Builtins) -> (MioConnector, Spawner) {
    let context = builtins.io().context::<MioContext>(&MioOptions::default());
    let connector = MioConnector::new(
        context,
        builtins.scheduler().clone(),
        builtins.clock().clone(),
        MioConnectorOptions::default(),
    );
    let spawner = Spawner::new_arty(builtins.scheduler().clone());
    (connector, spawner)
}
```


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/fetch_mio">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjNhdIQborR2_k_xJd4bTcf2krrNPIcbP72Pw1UdRjkbim_eMDe2BBthYvRhcoQbbX8Zk1o5YW4ba3wj9SbooMYbmASWACAngpgbyq0XOiOzfBVhZISCaGFueXNwYXduZTAuOS4ygmhhcnR5X21pb2UwLjEuMIJrZmV0Y2hfaHlwZXJmMC4xMC4wgmlmZXRjaF9taW9lMC4xLjA
 [__link0]: https://crates.io/crates/fetch_hyper/0.10.0
 [__link1]: https://docs.rs/fetch_mio/0.1.0/fetch_mio/?search=MioConnector
 [__link2]: https://crates.io/crates/arty_mio/0.1.0
 [__link3]: https://docs.rs/fetch_mio/0.1.0/fetch_mio/?search=MioStream
 [__link4]: https://docs.rs/fetch_hyper/0.10.0/fetch_hyper/?search=HyperIo
 [__link5]: https://docs.rs/arty_mio/0.1.0/arty_mio/?search=MioContext
 [__link6]: https://docs.rs/anyspawn/0.9.2/anyspawn/?search=Spawner::new_arty
 [__link7]: https://docs.rs/fetch_hyper/0.10.0/fetch_hyper/?search=HyperTransportBuilder
