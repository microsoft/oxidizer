<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Arty Macros Logo" width="96">

# Arty Macros

[![crate.io](https://img.shields.io/crates/v/arty_macros.svg)](https://crates.io/crates/arty_macros)
[![docs.rs](https://docs.rs/arty_macros/badge.svg)](https://docs.rs/arty_macros)
[![MSRV](https://img.shields.io/crates/msrv/arty_macros)](https://crates.io/crates/arty_macros)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml/badge.svg)](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Entry-point macros for [`arty`][__link0].

Enable the `macros` feature in Arty and use `#[arty::runtime::main]` or `#[arty::runtime::test]`.
Each annotated asynchronous function takes one owned `arty::runtime::Builtins` argument.
Use `runtime_path = ::renamed_arty::runtime` for a renamed or re-exported runtime.

* `workers = N` limits asynchronous workers to at most the nonzero integer literal `N`.
  This does not limit blocking-task pools or change the runtime’s default when omitted.
* `builder = expression` uses an existing `arty::runtime::RuntimeBuilder`, evaluated once
  on the calling thread. It cannot be combined with `workers`.
* Tests may opt into simulated time by adding an owned `arty::time::ClockControl` as
  their second parameter. This requires Arty’s `test-util` feature, starts with manual
  advancement, and cannot be combined with `builder`.

See the [`main`][__link1] and
[`test`][__link2] documentation in Arty
for examples, clock semantics, and construction-error behavior.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/arty_macros">source code</a>.
</sub>

 [__link0]: https://docs.rs/arty
 [__link1]: https://docs.rs/arty/latest/arty/runtime/attr.main.html
 [__link2]: https://docs.rs/arty/latest/arty/runtime/attr.test.html
