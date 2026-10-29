<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Arty Mio Logo" width="96">

# Arty Mio

[![crate.io](https://img.shields.io/crates/v/arty_mio.svg)](https://crates.io/crates/arty_mio)
[![docs.rs](https://docs.rs/arty_mio/badge.svg)](https://docs.rs/arty_mio)
[![MSRV](https://img.shields.io/crates/msrv/arty_mio)](https://crates.io/crates/arty_mio)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml/badge.svg)](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Mio I/O integration for the Arty runtime.

Each [`MioDriver`][__link0] owns a Mio [`Poll`][__link1] and its driver-side
lifecycle. A primary driver polls on the Arty worker. A secondary driver
uses a helper thread to wait for Mio events and wake the Arty worker.

[`MioContext`][__link2] selects the worker route for a new source. It registers the
source on the calling thread through a cloned Mio registry. The driver
creates the clone with `mio::Registry::try_clone`. [`MioContext::register`][__link3]
returns one [`Registered`][__link4] value that owns the source.

[`Registered`][__link5] provides ordinary shared and exclusive references to the
source. It does not put the source behind a mutex. Read and write readiness
use separate poll methods that return `()`. Readiness is a hint after an I/O
operation returns `io::ErrorKind::WouldBlock`. Driver shutdown closes
readiness, but the `Registered` owner retains and drops the source.

## Example

```rust
use std::io;

use arty::task::Builtins;
use arty_mio::{MioContext, MioOptions};
use mio::Interest;

async fn receive(builtins: &Builtins) -> io::Result<()> {
    let io = builtins.io().context::<MioContext>(&MioOptions::default());
    let address = "127.0.0.1:0".parse().expect("the example address is valid");
    let socket = mio::net::UdpSocket::bind(address)?;
    let mut registration = io.register(socket, Interest::READABLE)?;
    let mut buffer = [0; 1_024];

    loop {
        match registration
            .source_mut()
            .map_err(io::Error::other)?
            .recv(&mut buffer)
        {
            Ok(length) => {
                println!("received {length} bytes");
                return Ok(());
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                std::future::poll_fn(|context| registration.poll_read_ready(context))
                    .await
                    .map_err(io::Error::other)?;
            }
            Err(error) => return Err(error),
        }
    }
}
```


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/arty_mio">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjNhdIQborR2_k_xJd4bTcf2krrNPIcbP72Pw1UdRjkbim_eMDe2BBthYvRhcoQbbaCaxEkgr4IbeDlfhG65fEcbhCYt6LPRAb4bsjsNd_IMJJhhZIKCaGFydHlfbWlvZTAuMS4wgmNtaW9lMS4yLjM
 [__link0]: https://docs.rs/arty_mio/0.1.0/arty_mio/?search=MioDriver
 [__link1]: https://docs.rs/mio/1.2.3/mio/?search=Poll
 [__link2]: https://docs.rs/arty_mio/0.1.0/arty_mio/?search=MioContext
 [__link3]: https://docs.rs/arty_mio/0.1.0/arty_mio/?search=MioContext::register
 [__link4]: https://docs.rs/arty_mio/0.1.0/arty_mio/?search=Registered
 [__link5]: https://docs.rs/arty_mio/0.1.0/arty_mio/?search=Registered
