<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Fetch Winhttp Logo" width="96">

# Fetch Winhttp

[![crate.io](https://img.shields.io/crates/v/fetch_winhttp.svg)](https://crates.io/crates/fetch_winhttp)
[![docs.rs](https://docs.rs/fetch_winhttp/badge.svg)](https://docs.rs/fetch_winhttp)
[![MSRV](https://img.shields.io/crates/msrv/fetch_winhttp)](https://crates.io/crates/fetch_winhttp)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

WinHTTP-based HTTP transport for the [`fetch`][__link0] HTTP client.

This Windows-only crate adds a `WinHTTP` transport constructor to
[`HttpClient`][__link1]. Callers supply the clock, memory pool, and telemetry sink
required by the transport through [`WinHttpDeps`][__link2].

WinHTTP-specific TLS and timeout configuration is available through
[`WinHttpTlsConfig`][__link3] and [`WinHttpOptions`][__link4]. Independently built clients do
not share connections.

### Platform and transport behavior

* Windows 11 build 22000 or later is required.
* The transport currently follows automatic Windows proxy policy and exposes
  no proxy override or direct-connection fallback. Future requirements may
  add explicit control.
* Generic TLS configuration and generic transport options that `WinHTTP`
  cannot represent, including finite connection limits and connection
  idle/lifetime settings, are accepted but ignored.
* The request body is fully sent before response reception begins.
* Response trailers exposed by `WinHTTP` are preserved for HTTP/2 and HTTP/3.
  HTTP/1.1 permits trailer fields, but `WinHTTP` does not expose them.
  Request trailers are rejected.

Requests are serviced through the operating system’s
[WinHTTP][__link5]
API.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/fetch_winhttp">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjJhdIQb11VxC_uAPOQbtUn4Wx2-BfAbid3Nt1Y27Pobprn8Z6FjFy9hYvRhcoQbGdGXW4iTGGwb-8X2VDrDT6Ub3L57PFxe6-cbngYc-E3MdzhhZIGCbWZldGNoX3dpbmh0dHBlMC4xLjA
 [__link0]: https://docs.rs/fetch
 [__link1]: https://docs.rs/fetch
 [__link2]: https://docs.rs/fetch_winhttp/0.1.0/fetch_winhttp/?search=WinHttpDeps
 [__link3]: https://docs.rs/fetch_winhttp/0.1.0/fetch_winhttp/?search=WinHttpTlsConfig
 [__link4]: https://docs.rs/fetch_winhttp/0.1.0/fetch_winhttp/?search=WinHttpOptions
 [__link5]: https://learn.microsoft.com/en-us/windows/win32/winhttp/using-winhttp
