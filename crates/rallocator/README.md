<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Rallocator Logo" width="96">

# Rallocator

[![crate.io](https://img.shields.io/crates/v/rallocator.svg)](https://crates.io/crates/rallocator)
[![docs.rs](https://docs.rs/rallocator/badge.svg)](https://docs.rs/rallocator)
[![MSRV](https://img.shields.io/crates/msrv/rallocator)](https://crates.io/crates/rallocator)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml/badge.svg)](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

An experimental Rust global allocator for NUMA and mostly thread-local workloads.

We use it to experiment with allocation hinting, compile-time configuration,
and runtime telemetry through `seismograph`.

For now, it is a shameless Rust port of
[snmalloc][__link0], but the implementation has
changed before and will probably change again.

## Why use it

The design targets allocation-heavy multithreaded and thread-per-core
workloads: keep ordinary allocations local and amortize cross-thread frees.
Built-in Seismograph integration can record allocation events and expose
allocator state for diagnosing memory use. Enable recording through
[`seismograph::recorder()`][__link1]
when diagnostics are needed.

## Usage

Add the dependency:

```toml
[dependencies]
rallocator = "0.2"
```

Call [`rallocator!`][__link2] once in your executable to install the global allocator:

```rust
rallocator::rallocator!();
```

Compile-time allocator configuration has historically been cumbersome to
express in a manually declared `#[global_allocator]` static. The macro provides
an installation facade that can be extended with configuration options later.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/rallocator">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjNhdIQborR2_k_xJd4bTcf2krrNPIcbP72Pw1UdRjkbim_eMDe2BBthYvRhcoQbA5kLf_QRxBobNIEVKfwWQqEbWHO4Khw32P0bShTZ4JBR1WBhZIGCanJhbGxvY2F0b3JlMC4zLjE
 [__link0]: https://github.com/microsoft/snmalloc
 [__link1]: https://docs.rs/seismograph/latest/seismograph/fn.recorder.html
 [__link2]: https://docs.rs/rallocator/0.3.1/rallocator/macro.rallocator.html
