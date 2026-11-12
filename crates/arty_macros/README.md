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

Attribute macros for [`arty`][__link0] runtime entry points.

Enable the `macros` feature of Arty. Then use `#[arty::main]` for an
application or `#[arty::test]` for a test. You do not need to add a direct
dependency on this companion crate.

Both attributes start an Arty runtime. They pass an
[`arty::task::Context`][__link1]
to the async function. The runtime stops when the function returns.

`workers = N` starts exactly `N` async workers. If `N` exceeds the available
processor capacity, multiple workers share processors. `workers = all`
starts one worker for each available processor. Both attributes use one
worker by default.

`monitor = enabled` or `monitor = disabled` selects whether the runtime is
prepared for monitoring. `#[arty::main]` enables it by default.
`#[arty::test]` disables it by default. `panic = shutdown` or
`panic = ignore` selects whether the runtime stops after Arty catches a task
panic. Both attributes use `shutdown` by default.

See [`arty::main`][__link2] and
[`arty::test`][__link3] for signatures,
configuration, examples, and failure conditions.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/arty_macros">source code</a>.
</sub>

 [__link0]: https://docs.rs/arty
 [__link1]: https://docs.rs/arty/latest/arty/task/struct.Context.html
 [__link2]: https://docs.rs/arty/latest/arty/attr.main.html
 [__link3]: https://docs.rs/arty/latest/arty/attr.test.html
