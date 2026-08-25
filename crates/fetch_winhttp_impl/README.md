<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Fetch Winhttp Impl Logo" width="96">

# Fetch Winhttp Impl

[![crate.io](https://img.shields.io/crates/v/fetch_winhttp_impl.svg)](https://crates.io/crates/fetch_winhttp_impl)
[![docs.rs](https://docs.rs/fetch_winhttp_impl/badge.svg)](https://docs.rs/fetch_winhttp_impl)
[![MSRV](https://img.shields.io/crates/msrv/fetch_winhttp_impl)](https://crates.io/crates/fetch_winhttp_impl)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Implementation of the [`fetch_winhttp`][__link0] WinHTTP transport.

This crate carries the transport itself; [`fetch_winhttp`][__link1] is the package to
depend on, and re-exports everything here that is supported. Nothing in this
crate is a supported API: items are public only so that the facade can
re-export them, and they may be changed or removed at any time.

The crate is empty on targets other than Windows.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/fetch_winhttp_impl">source code</a>.
</sub>

 [__link0]: https://docs.rs/fetch_winhttp
 [__link1]: https://docs.rs/fetch_winhttp
