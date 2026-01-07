<div align="center">
 <img src="./logo.png" alt="Thread Aware Macros Logo" width="96">

# Thread Aware Macros

[![crate.io](https://img.shields.io/crates/v/thread_aware_macros.svg)](https://crates.io/crates/thread_aware_macros)
[![docs.rs](https://docs.rs/thread_aware_macros/badge.svg)](https://docs.rs/thread_aware_macros)
[![MSRV](https://img.shields.io/crates/msrv/thread_aware_macros)](https://crates.io/crates/thread_aware_macros)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](../../LICENSE)
<a href="../.."><img src="../../logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Macros for the [`thread_aware`][__link0] crate.

## Provided Derives

* `#[derive(ThreadAware)]`: Auto-implements the `thread_aware::ThreadAware` trait by recursively
  calling `transfer` on each field.


<hr/>
<sub>
This crate was developed as part of <a href="../..">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/thread_aware_macros">source code</a>.
</sub>

 [__link0]: https://docs.rs/thread_aware
