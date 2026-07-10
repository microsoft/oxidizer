<div align="center">
 <img src="./logo.png" alt="Fetch Grpc Logo" width="96">

# Fetch Grpc

[![crate.io](https://img.shields.io/crates/v/fetch_grpc.svg)](https://crates.io/crates/fetch_grpc)
[![docs.rs](https://docs.rs/fetch_grpc/badge.svg)](https://docs.rs/fetch_grpc)
[![MSRV](https://img.shields.io/crates/msrv/fetch_grpc)](https://crates.io/crates/fetch_grpc)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](../../LICENSE)
<a href="../.."><img src="../../logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

A gRPC transport backed by the `fetch` HTTP client.

This crate adapts a `fetch` HTTP client into a transport for the
[`grpc`][__link0] crate, so gRPC calls run over `fetch` and
benefit from its resilience and observability.

It is currently an empty placeholder. A real implementation will follow.


<hr/>
<sub>
This crate was developed as part of <a href="../..">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/fetch_grpc">source code</a>.
</sub>

 [__link0]: https://docs.rs/grpc
