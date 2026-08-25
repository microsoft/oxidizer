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

This Windows-only crate adds a WinHTTP transport constructor to
[`HttpClient`][__link1]. Callers supply the clock, memory pool, and telemetry sink
required by the transport through [`WinHttpDeps`][__link2].

WinHTTP-specific TLS configuration is available through
[`WinHttpTlsConfig`][__link3]. Independently built clients do
not share connections.

### Platform requirements

Windows 11 (build 22000) or later, or Windows Server 2025 (build 26100) or
later. Windows Server 2022 and earlier are not supported.

### Behavior and limitations

WinHTTP owns connection management and much of the HTTP protocol, so some
generic `fetch` configuration cannot be represented and some behavior is
fixed by the operating system.

* Generic TLS configuration, finite connection limits, and bounded
  connection lifetimes are accepted and ignored rather than rejected, so a
  client that sets them still builds.
* Proxy selection follows automatic Windows proxy policy, including
  automatic discovery and proxy auto-configuration scripts. Callers cannot
  override that policy, and it may route a request through a proxy or
  send it directly to the origin.
* Redirects are not followed, no cookie store is kept, and authentication
  challenges are not answered. Those responses are returned to the caller to
  act on, and none of this can be re-enabled.
* Request framing is derived from the body, so a caller-supplied
  `Transfer-Encoding` is rejected before anything is sent. Code that
  forwards an inbound request’s headers verbatim is the common case that
  trips on this.
* Gzip and deflate responses are decoded transparently, with the headers
  describing the encoded form removed and no opt-out. Brotli and zstd are
  not decoded and arrive still encoded.
* A request body that yields trailers fails the request, and does so after
  the headers and preceding body data have already been sent. The request
  body is sent in full before response reception begins.

Failures carry a stable error label and retry guidance. The label is
contractual; the retry guidance attached to any particular failure is not,
and may change as the transport’s classification is refined.

The full contract - error classification, timeout semantics, and the
fidelity of every generic option - is documented in
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
