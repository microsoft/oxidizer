<div align="center">
 <img src="./logo.png" alt="Observed Macros Logo" width="96">

# Observed Macros

[![crate.io](https://img.shields.io/crates/v/observed_macros.svg)](https://crates.io/crates/observed_macros)
[![docs.rs](https://docs.rs/observed_macros/badge.svg)](https://docs.rs/observed_macros)
[![MSRV](https://img.shields.io/crates/msrv/observed_macros)](https://crates.io/crates/observed_macros)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](../../LICENSE)
<a href="../.."><img src="../../logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Procedural macros for the `observed` crate.

This crate provides:

* `#[derive(Event)]` - generate an `Event` trait impl for a struct
* `#[derive(Enrichment)]` - generate an `Enrichment` trait impl for a struct

**Do not depend on this crate directly.** Use the re-exports from `observed` instead.


<hr/>
<sub>
This crate was developed as part of <a href="../..">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/observed_macros">source code</a>.
</sub>

