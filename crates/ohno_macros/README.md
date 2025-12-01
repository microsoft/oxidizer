<div align="center">
 <img src="./logo.png" alt="Ohno Macros Logo" width="128">

# Ohno Macros

[![crate.io](https://img.shields.io/crates/v/ohno_macros.svg)](https://crates.io/crates/ohno_macros)
[![docs.rs](https://docs.rs/ohno_macros/badge.svg)](https://docs.rs/ohno_macros)
[![MSRV](https://img.shields.io/crates/msrv/ohno_macros)](https://crates.io/crates/ohno_macros)
[![CI](https://github.com/microsoft/oxidizer/workflows/main/badge.svg)](https://github.com/microsoft/oxidizer/actions)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](../LICENSE)

</div>

* [Summary](#summary)

## Summary

<!-- cargo-rdme start -->

Macros for the [`ohno`](https://docs.rs/ohno) crate.

## Macros

- `#[derive(Error)]` - Automatically implement error traits
- `#[enrich_err("message")]` - Add error trace with file/line information to function errors

<!-- cargo-rdme end -->

<div style="font-size: 75%" ><hr/>

This crate was developed as part of [The Oxidizer Project](https://github.com/microsoft/oxidizer).

</div>
