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

WinHTTP-specific TLS and timeout configuration is available through
[`WinHttpTlsConfig`][__link3] and [`WinHttpOptions`][__link4]. Independently built clients do
not share connections.

### Platform and transport behavior

* Windows 11 build 22000 or later is required.
* Proxy selection follows automatic Windows proxy policy, including
  automatic discovery and proxy auto-configuration scripts; no proxy
  override or direct-connection fallback is exposed.
* The connection idle timeout is honored, raised to a platform minimum when
  the caller asks for a shorter window. An unlimited idle timeout is
  approximated by the longest window the platform can express, which exceeds
  forty-nine days.
* Generic TLS configuration and the generic transport options WinHTTP
  cannot represent, including finite connection limits and bounded
  connection lifetimes, are accepted but ignored.
* The request body is fully sent before response reception begins.
* A request carrying a `Transfer-Encoding` header is rejected before
  anything is sent, because this transport performs request framing itself
  and cannot honor a caller-supplied transfer coding. Removing the header
  does not change how the body is framed on the wire.
* A `Content-Length` header must be a single well-formed value, and
  repeated values must agree with each other. When the request body reports
  its own length, the header must equal it, and a disagreement fails the
  request before anything is sent. When the body cannot report a length, the
  header declares it and is taken on trust. A header that survives is sent
  in normalized decimal form.
* Redirects are not followed, no cookie store is kept, and authentication
  challenges are not answered automatically. A redirect response is
  returned to the caller as an ordinary response, and `Set-Cookie`,
  `Cookie`, and challenge headers pass through as plain data for the caller
  to act on. None of these can be re-enabled.
* Gzip and deflate response bodies are decoded transparently, and the
  `Content-Encoding` and `Content-Length` headers describing the encoded
  form are removed. There is no opt-out. Brotli and zstd responses are
  delivered still encoded, with their headers intact.
* Response trailers exposed by WinHTTP are preserved for HTTP/2 and HTTP/3.
  HTTP/1.1 permits trailer fields, but WinHTTP does not expose them.
  Request trailers are rejected, and because a trailer frame is reached only
  once the body yields it, that rejection arrives after the headers and any
  preceding body data have been sent.

Requests are serviced through the operating system’s
[WinHTTP][__link5]
API.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/fetch_winhttp">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjJhdIQb11VxC_uAPOQbtUn4Wx2-BfAbid3Nt1Y27Pobprn8Z6FjFy9hYvRhcoQb05Y4yz2EhSUb_WUj-fedRzMbJzzLIW6JSRUbChzaHWEeGElhZIGCbWZldGNoX3dpbmh0dHBlMC4xLjA
 [__link0]: https://docs.rs/fetch
 [__link1]: https://docs.rs/fetch
 [__link2]: https://docs.rs/fetch_winhttp/0.1.0/fetch_winhttp/?search=WinHttpDeps
 [__link3]: https://docs.rs/fetch_winhttp/0.1.0/fetch_winhttp/?search=WinHttpTlsConfig
 [__link4]: https://docs.rs/fetch_winhttp/0.1.0/fetch_winhttp/?search=WinHttpOptions
 [__link5]: https://learn.microsoft.com/en-us/windows/win32/winhttp/using-winhttp
