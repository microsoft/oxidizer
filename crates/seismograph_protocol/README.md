<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Seismograph Protocol Logo" width="96">

# Seismograph Protocol

[![crate.io](https://img.shields.io/crates/v/seismograph_protocol.svg)](https://crates.io/crates/seismograph_protocol)
[![docs.rs](https://docs.rs/seismograph_protocol/badge.svg)](https://docs.rs/seismograph_protocol)
[![MSRV](https://img.shields.io/crates/msrv/seismograph_protocol)](https://crates.io/crates/seismograph_protocol)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml/badge.svg)](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Local monitor protocol, OS-authorized transport, and discovery for Seismograph.

Monitors use pathname Unix sockets or local Windows named pipes. Connections
are authorized by the operating system for the owning user and root/elevated
administrators, without authentication secrets. A token-free Hello identifies
the process instance and returns its recording configuration.

[`discovery::Directory::current`][__link0] automatically creates a private registry
for the current user, without administrator setup. Unix prefers
`$XDG_RUNTIME_DIR/seismograph`, with a private randomized `/tmp` directory
as the fallback. Windows uses local application data. Discovery aggregates
accessible user registries; unsafe existing permissions are never relaxed.
Monitor protocol version 8 is incompatible with the earlier TCP/token
protocol; saved telemetry snapshots have an independent, unchanged format.

Construct an [`io::MonitorIo`][__link1] at the application boundary and pass it to
discovery, descriptor, and transport constructors. Their returned objects
retain that capability.

## Features

* **`test-util`**: Provides supported, documented mock I/O through
  `io::MonitorIo::new_mocked` and `io::MonitorIoControl`, including isolated
  files and streams, injected failures, partial transfers, and controlled clocks.
  Enable this on a dev-dependency for application tests. Mocked I/O creates no
  host files or endpoints and does not prove native OS permission enforcement.
  The same feature enables `testing` for isolated native IPC tests, which
  create real temporary files and OS endpoints without using the installed
  user registries.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/seismograph_protocol">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjNhdIQborR2_k_xJd4bTcf2krrNPIcbP72Pw1UdRjkbim_eMDe2BBthYvRhcoQbGM2JhyQTxREbAqQLjm1wkmYb0mVr1UrJwjgb6YJn06NaBMNhZIGCdHNlaXNtb2dyYXBoX3Byb3RvY29sZTAuMi4w
 [__link0]: https://docs.rs/seismograph_protocol/0.2.0/seismograph_protocol/?search=discovery::Directory::current
 [__link1]: https://docs.rs/seismograph_protocol/0.2.0/seismograph_protocol/?search=io::MonitorIo
