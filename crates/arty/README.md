<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Arty Logo" width="96">

# Arty

[![crate.io](https://img.shields.io/crates/v/arty.svg)](https://crates.io/crates/arty)
[![docs.rs](https://docs.rs/arty/badge.svg)](https://docs.rs/arty)
[![MSRV](https://img.shields.io/crates/msrv/arty)](https://crates.io/crates/arty)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Single-threaded, thread-aware application runtime.

Arty is being developed as a small runtime whose foundational contracts live in
[`arty_core`][__link0]. Its public surface is intentionally limited while those contracts are being
established.

## Features

* **`time`** - Exposes time primitives through [`time`][__link1].
* **`test-util`** - Exposes `time::ClockControl` when `time` is also enabled.

## Project policies

* [Design][__link2]
* [I/O][__link3]
* [Panics][__link4]
* [Stabilization][__link5]


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/arty">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjJhdIQb11VxC_uAPOQbtUn4Wx2-BfAbid3Nt1Y27Pobprn8Z6FjFy9hYvRhcoQbEFhdCMfrIyQbub_8bikp6rcbiSrLDhTe6HsbMdTJ8_3SS1phZIKCZGFydHllMC4yLjCCaWFydHlfY29yZWUwLjIuMA
 [__link0]: https://crates.io/crates/arty_core/0.2.0
 [__link1]: https://docs.rs/arty/0.2.0/arty/time/index.html
 [__link2]: https://github.com/microsoft/oxidizer/blob/main/crates/arty/docs/DESIGN.md
 [__link3]: https://github.com/microsoft/oxidizer/blob/main/crates/arty/docs/IO.md
 [__link4]: https://github.com/microsoft/oxidizer/blob/main/crates/arty/docs/PANICS.md
 [__link5]: https://github.com/microsoft/oxidizer/blob/main/crates/arty/docs/STABILIZATION.md
