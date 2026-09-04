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

* **`time`** - Exposes time primitives through `arty::time`.
* **`test-util`** - Exposes `time::ClockControl` when `time` is also enabled.

## Project policies

* [Design][__link1]
* [I/O][__link2]
* [Panics][__link3]
* [Stabilization][__link4]


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/arty">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjJhdIQb11VxC_uAPOQbtUn4Wx2-BfAbid3Nt1Y27Pobprn8Z6FjFy9hYvRhcoQbVwyeVLFSO68b1R3yOQcfYiwbakqCi1YB3uIbJnq7wcCXYGJhZIGCaWFydHlfY29yZWUwLjIuMA
 [__link0]: https://crates.io/crates/arty_core/0.2.0
 [__link1]: https://github.com/microsoft/oxidizer/blob/main/crates/arty/docs/DESIGN.md
 [__link2]: https://github.com/microsoft/oxidizer/blob/main/crates/arty/docs/IO.md
 [__link3]: https://github.com/microsoft/oxidizer/blob/main/crates/arty/docs/PANICS.md
 [__link4]: https://github.com/microsoft/oxidizer/blob/main/crates/arty/docs/STABILIZATION.md
