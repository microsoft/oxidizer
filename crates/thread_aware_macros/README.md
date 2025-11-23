<div align="center">
 <img src="./logo.png" alt="Thread Aware Macros Logo" width="128">

# Thread Aware Macros

[![crate.io](https://img.shields.io/crates/v/thread_aware_macros.svg)](https://crates.io/crates/thread_aware_macros)
[![docs.rs](https://docs.rs/thread_aware_macros/badge.svg)](https://docs.rs/thread_aware_macros)
[![MSRV](https://img.shields.io/crates/msrv/thread_aware_macros)](https://crates.io/crates/thread_aware_macros)
[![CI](https://github.com/microsoft/oxidizer/workflows/main/badge.svg)](https://github.com/microsoft/oxidizer/actions)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](../LICENSE)

</div>

* [Summary](#summary)
* [Provided Derives](#provided-derives)

## Summary

<!-- cargo-rdme start -->

Macros for the [`thread_aware`](https://docs.rs/thread_aware) crate.

## Provided Derives

* `#[derive(ThreadAware)]` – Auto-implements the `thread_aware::ThreadAware` trait by recursively
  calling `transfer` on each field.

<!-- cargo-rdme end -->

<div style="font-size: 75%" ><hr/>

This crate was developed as part of [The Oxidizer Project](https://github.com/microsoft/oxidizer).

</div>
