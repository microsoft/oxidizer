<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Seismograph Cli Logo" width="96">

# Seismograph Cli

[![crate.io](https://img.shields.io/crates/v/seismograph_cli.svg)](https://crates.io/crates/seismograph_cli)
[![docs.rs](https://docs.rs/seismograph_cli/badge.svg)](https://docs.rs/seismograph_cli)
[![MSRV](https://img.shields.io/crates/msrv/seismograph_cli)](https://crates.io/crates/seismograph_cli)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Live monitoring, interactive snapshot viewing, and HTML reporting for `seismograph`.

The CLI renders common thread, stack, and runtime-event data directly.
Rallocator payloads use the built-in schema-specific renderer; unknown
sources remain visible in the source inventory.

Run `seismograph monitor` to capture a running application, or
`seismograph view "C:\captures\capture.seismograph"` to inspect a native
snapshot in the same interactive tabs without connecting to a process.
The offline viewer is read-only: use `Tab` or `1` through `8` to select tabs,
the usual arrow/Enter/Backspace navigation within tabs, and `q` or `Esc` to quit.
Large files load on a worker thread while the terminal remains responsive.
Loading still requires memory for the decoded events and their summaries.
Snapshot files do not record a wall-clock capture time.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/seismograph_cli">source code</a>.
</sub>

