<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Fetch Winhttp Logo" width="96">

# Fetch Winhttp

[![crate.io](https://img.shields.io/crates/v/fetch_winhttp.svg)](https://crates.io/crates/fetch_winhttp)
[![docs.rs](https://docs.rs/fetch_winhttp/badge.svg)](https://docs.rs/fetch_winhttp)
[![MSRV](https://img.shields.io/crates/msrv/fetch_winhttp)](https://crates.io/crates/fetch_winhttp)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml/badge.svg)](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

WinHTTP transport for the [`fetch`][__link0] HTTP client.

This Windows-only crate adds a WinHTTP constructor to [`HttpClient`][__link1].
[`WinHttpDeps`][__link2] supplies the clock, memory provider, and telemetry sink.
The memory provider uses `bytesbuf::mem::OpaqueMemory`, so callers can select
an allocator.

[`WinHttpTlsConfig`][__link3] provides WinHTTP-specific TLS configuration.
Independently built clients do not share connections.

### Platform requirements

Windows 11 (build 22000) or later, or Windows Server 2025 (build 26100) or
later. Windows Server 2022 and earlier are not supported.

### Behavior and limitations

WinHTTP manages connections and much of the HTTP protocol. Some generic
`fetch` configuration does not apply, and Windows fixes some behavior.

* Generic TLS configuration, finite connection limits, and bounded
  connection lifetimes are accepted but ignored. A client that sets them
  still builds.
* Requests always connect directly to the origin. Proxies are not supported,
  Windows proxy configuration is not consulted, and there is no setting that
  changes this.
* Redirects are not followed. The transport does not keep cookies or answer
  authentication challenges. It returns these responses to the caller.
* The body determines request framing. A caller-supplied
  `Transfer-Encoding` is rejected before the transport sends data.
* Gzip and deflate responses are decoded automatically. The transport
  removes headers that describe the encoded form. Brotli and zstd responses
  remain encoded.
* Request trailers fail the request after the transport sends the headers
  and earlier body data. The transport sends the full request body before it
  receives the response.

Failures include a stable error label and retry guidance. The label is part
of the contract. The retry guidance can change.

The full contract covers error classification, timeout behavior, and each
generic configuration value. It is documented in
[`docs/design.md`][__link4].

Requests are serviced through the operating system’s
[WinHTTP][__link5]
API.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/fetch_winhttp">source code</a>.
</sub>

 [__link0]: https://docs.rs/fetch
 [__link1]: https://docs.rs/fetch
 [__link2]: https://docs.rs/fetch_winhttp/latest/fetch_winhttp/struct.WinHttpDeps.html
 [__link3]: https://docs.rs/fetch_winhttp/latest/fetch_winhttp/struct.WinHttpTlsConfig.html
 [__link4]: https://github.com/microsoft/oxidizer/blob/main/crates/fetch_winhttp/docs/design.md
 [__link5]: https://learn.microsoft.com/en-us/windows/win32/winhttp/using-winhttp
