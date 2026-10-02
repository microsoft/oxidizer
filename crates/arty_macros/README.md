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

Enable Arty’s `macros` feature and use `#[arty::main]` or `#[arty::test]`.
You do not need to depend on this companion crate directly.

Both attributes start a runtime, pass its worker capabilities to an
async function, and shut down when that function returns. Options
select a worker limit, a custom runtime builder, or a renamed runtime module.

The application-facing references are
[`arty::main`][__link1] and
[`arty::test`][__link2]. They contain
runnable examples, configuration syntax, and failure conditions.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/arty_macros">source code</a>.
</sub>

 [__link0]: https://docs.rs/arty
 [__link1]: https://docs.rs/arty/latest/arty/attr.main.html
 [__link2]: https://docs.rs/arty/latest/arty/attr.test.html
