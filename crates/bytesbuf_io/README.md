<div align="center">
 <img src="./logo.png" alt="Bytesbuf Io Logo" width="96">

# Bytesbuf Io

[![crate.io](https://img.shields.io/crates/v/bytesbuf_io.svg)](https://crates.io/crates/bytesbuf_io)
[![docs.rs](https://docs.rs/bytesbuf_io/badge.svg)](https://docs.rs/bytesbuf_io)
[![MSRV](https://img.shields.io/crates/msrv/bytesbuf_io)](https://crates.io/crates/bytesbuf_io)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](../../LICENSE)
<a href="../.."><img src="../../logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Asynchronous I/O abstractions expressed via `bytesbuf` types.

These types model byte sources that can be read from ([`Read`][__link0] trait) and byte sinks that can be
written to ([`Write`][__link1] trait). All operations use byte sequences represented by types from
`bytesbuf` instead of raw byte slices, enabling the level of flexibility required for
implementing and using high-performance I/O endpoints that consume or produce byte streams.

All operations are asynchronous and take ownership of the data/buffers passed to them,
enabling efficient implementation of high-performance I/O endpoints with zero-copy semantics.

The `futures-stream` feature enables integration with the `futures` crate, providing
an adapter that exposes a [`Read`][__link2] implementation as a `futures::Stream` of byte sequences.

The `test-util` feature enables additional utilities for testing implementations of
types that produce or consume streams of bytes. These are in the `testing` module.


<hr/>
<sub>
This crate was developed as part of <a href="../..">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/bytesbuf_io">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGkYW0CYXSEGy4k8ldDFPOhG2VNeXtD5nnKG6EPY6OfW5wBG8g18NOFNdxpYXKEGwmc-588ToSEGyIu1q27bO0yG_keVxaeiTxJG5Uncv8WLD-8YWSBgmtieXRlc2J1Zl9pb2UwLjEuMQ
 [__link0]: https://docs.rs/bytesbuf_io/0.1.1/bytesbuf_io/?search=Read
 [__link1]: https://docs.rs/bytesbuf_io/0.1.1/bytesbuf_io/?search=Write
 [__link2]: https://docs.rs/bytesbuf_io/0.1.1/bytesbuf_io/?search=Read
