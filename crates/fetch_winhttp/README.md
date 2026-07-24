<div align="center">
 <img src="./logo.png" alt="Fetch Winhttp Logo" width="96">

# Fetch Winhttp

[![crate.io](https://img.shields.io/crates/v/fetch_winhttp.svg)](https://crates.io/crates/fetch_winhttp)
[![docs.rs](https://docs.rs/fetch_winhttp/badge.svg)](https://docs.rs/fetch_winhttp)
[![MSRV](https://img.shields.io/crates/msrv/fetch_winhttp)](https://crates.io/crates/fetch_winhttp)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](../../LICENSE)
<a href="../.."><img src="../../logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

WinHTTP-based HTTP transport for the [`fetch`][__link0] HTTP client.

This crate is a Windows-only custom transport that services `fetch`
[`HttpClient`][__link1] requests through the operating system’s
[WinHTTP][__link2]
API, running in fully asynchronous mode.

## Status

This crate is a placeholder. Only the design exists so far; there is no
implementation yet. See [`docs/DESIGN.md`][__link3]
for the proposed architecture, threading/cancellation/error models, and the
test plan.


<hr/>
<sub>
This crate was developed as part of <a href="../..">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/fetch_winhttp">source code</a>.
</sub>

 [__link0]: https://docs.rs/fetch
 [__link1]: https://docs.rs/fetch
 [__link2]: https://learn.microsoft.com/en-us/windows/win32/winhttp/using-winhttp
 [__link3]: https://github.com/microsoft/oxidizer/blob/main/crates/fetch_winhttp/docs/DESIGN.md
