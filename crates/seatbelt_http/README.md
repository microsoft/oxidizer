<div align="center">
 <img src="./logo.png" alt="Seatbelt Http Logo" width="96">

# Seatbelt Http

[![crate.io](https://img.shields.io/crates/v/seatbelt_http.svg)](https://crates.io/crates/seatbelt_http)
[![docs.rs](https://docs.rs/seatbelt_http/badge.svg)](https://docs.rs/seatbelt_http)
[![MSRV](https://img.shields.io/crates/msrv/seatbelt_http)](https://crates.io/crates/seatbelt_http)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](../../LICENSE)
<a href="../.."><img src="../../logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

HTTP-specific extensions for the [`seatbelt`][__link0] resilience middleware.

Each [`seatbelt`][__link1] middleware is generic over its input and output types.
This crate specializes them for [`HttpRequest`][__link2] /
[`Result<HttpResponse>`][__link3] and adds HTTP-aware
builder methods, all prefixed with `http_`.

## Supported middleware

Each middleware lives in its own feature-gated module with specialized
type aliases and an extension trait:

|Module|Feature|Purpose|
|------|-------|-------|
|`retry`|`retry`|Recovery classification, request cloning, request restoration from errors.|
|`timeout`|`timeout`|Converts timeout events into HTTP-specific errors.|
|`hedging`|`hedging`|Recovery classification and request cloning for tail-latency reduction.|
|`breaker`|`breaker`|Recovery classification and rejected-request error handling.|

## Shared types

* [`HttpRecovery`][__link4]: classifies HTTP responses as recoverable. By default,
  5xx status codes, `429 Too Many Requests`, and request timeouts are
  treated as transient.
* [`HttpClone`][__link5]: selects which HTTP methods are eligible for cloning
  during retries and hedging (safe-only, idempotent, or all).
* [`HttpResilienceContext`][__link6]: the HTTP specialization of
  [`ResilienceContext`][__link7].


<hr/>
<sub>
This crate was developed as part of <a href="../..">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/seatbelt_http">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGkYW0CYXSEGy4k8ldDFPOhG2VNeXtD5nnKG6EPY6OfW5wBG8g18NOFNdxpYXKEG0zpOHumdJRlG7vvc1rvGGXMG8lb6BInDEwlGyUJKXppjYapYWSDgm9odHRwX2V4dGVuc2lvbnNlMC4zLjKCaHNlYXRiZWx0ZTAuNS4wgm1zZWF0YmVsdF9odHRwZTAuMS4w
 [__link0]: https://crates.io/crates/seatbelt/0.5.0
 [__link1]: https://crates.io/crates/seatbelt/0.5.0
 [__link2]: https://docs.rs/http_extensions/0.3.2/http_extensions/?search=HttpRequest
 [__link3]: https://docs.rs/http_extensions/0.3.2/http_extensions/?search=Result
 [__link4]: https://docs.rs/seatbelt_http/0.1.0/seatbelt_http/?search=HttpRecovery
 [__link5]: https://docs.rs/seatbelt_http/0.1.0/seatbelt_http/?search=HttpClone
 [__link6]: https://docs.rs/seatbelt_http/0.1.0/seatbelt_http/type.HttpResilienceContext.html
 [__link7]: https://docs.rs/seatbelt/0.5.0/seatbelt/?search=ResilienceContext
