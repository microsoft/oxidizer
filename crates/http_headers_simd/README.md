<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Http Headers Simd Logo" width="96">

# Http Headers Simd

[![crate.io](https://img.shields.io/crates/v/http_headers_simd.svg)](https://crates.io/crates/http_headers_simd)
[![docs.rs](https://docs.rs/http_headers_simd/badge.svg)](https://docs.rs/http_headers_simd)
[![MSRV](https://img.shields.io/crates/msrv/http_headers_simd)](https://crates.io/crates/http_headers_simd)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

SIMD implementation details for the
[`http_headers`][__link0] crate.

**Do not depend on this crate directly.** Use `http_headers` instead.

The default `std` feature enables runtime CPU-feature detection.
With default features disabled, dispatch uses compile-time target features
and architecture-specific fallbacks. The `benchmarking` and `test-util`
features expose unstable repository instrumentation only.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/http_headers_simd">source code</a>.
</sub>

 [__link0]: https://docs.rs/http_headers
