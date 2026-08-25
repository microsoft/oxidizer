<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Ohno Macros Impl Logo" width="96">

# Ohno Macros Impl

[![crate.io](https://img.shields.io/crates/v/ohno_macros_impl.svg)](https://crates.io/crates/ohno_macros_impl)
[![docs.rs](https://docs.rs/ohno_macros_impl/badge.svg)](https://docs.rs/ohno_macros_impl)
[![MSRV](https://img.shields.io/crates/msrv/ohno_macros_impl)](https://crates.io/crates/ohno_macros_impl)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Implementation of the procedural macros for the [`ohno`][__link0] crate.

This crate holds the logic behind:

* `#[derive(Error)]` - Automatically implement error traits
* `#[enrich_err("message")]` - Add error enrichment with file/line information to function errors
* `#[ohno::error]` - Turn a plain struct into an error type

**Do not depend on this crate directly.** Use the re-exports from `ohno` instead.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/ohno_macros_impl">source code</a>.
</sub>

 [__link0]: https://docs.rs/ohno
