<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Arty Macros Impl Logo" width="96">

# Arty Macros Impl

[![crate.io](https://img.shields.io/crates/v/arty_macros_impl.svg)](https://crates.io/crates/arty_macros_impl)
[![docs.rs](https://docs.rs/arty_macros_impl/badge.svg)](https://docs.rs/arty_macros_impl)
[![MSRV](https://img.shields.io/crates/msrv/arty_macros_impl)](https://crates.io/crates/arty_macros_impl)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml/badge.svg)](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Expands Arty’s runtime entry-point attributes.

This crate implements the attributes from
[`arty_macros`][__link0]. Application code uses Arty’s
`macros` feature and its `main` and `test` attributes.

[`main()`][__link1] turns an async function into a synchronous entry point. [`test()`][__link2]
also registers the function with Rust’s test harness. Invalid input produces
compiler diagnostic tokens.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/arty_macros_impl">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjNhdIQborR2_k_xJd4bTcf2krrNPIcbP72Pw1UdRjkbim_eMDe2BBthYvRhcoQbHO6d1u3kh8Yb2OIRyiQ1n-EbwxjHx3oKg-MbH3IJL5nTfNZhZIGCcGFydHlfbWFjcm9zX2ltcGxlMC42LjE
 [__link0]: https://docs.rs/arty_macros
 [__link1]: https://docs.rs/arty_macros_impl/0.6.1/arty_macros_impl/fn.main.html
 [__link2]: https://docs.rs/arty_macros_impl/0.6.1/arty_macros_impl/fn.test.html
