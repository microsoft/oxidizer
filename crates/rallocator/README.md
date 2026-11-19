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

We use it to experiment with allocation hinting, backend selection,
and runtime telemetry through `seismograph`.

The default backend is a Rust port of [snmalloc][__link0].
An experimental Rust port of [mimalloc][__link1]
is also available.

## Why use it

The design targets allocation-heavy multithreaded and thread-per-core
workloads: keep ordinary allocations local and amortize cross-thread frees.
Built-in Seismograph integration can record allocation events and expose
allocator state for diagnosing memory use. Enable recording through
[`seismograph::recorder()`][__link2]
when diagnostics are needed.

## Usage

Add the dependency:

```toml
[dependencies]
rallocator = "0.4"
```

Call [`rallocator!`][__link3] once in your executable to install the global allocator:

```rust
rallocator::rallocator!();
```

To fix the backend at compile time, use `rallocator!(backend = snmalloc)` or `rallocator!(backend = mimalloc)`.
Neither form reads the environment. The default remains snmalloc.

To use the same executable with different allocators, install the dynamic backend:

```rust
rallocator::rallocator!(backend = dynamic);
```

Before starting the executable, set `RALLOCATOR_BACKEND` to `mimalloc`, `snmalloc`, or `system`.
The first allocation fixes the choice for the life of the process.
An unset or empty value selects snmalloc. For any other unrecognized value, the allocator warns on standard error and uses snmalloc.

All three choices record allocation events when recording is enabled.
Only the snmalloc and mimalloc backends provide Seismograph state snapshots.
State observations can be incomplete or stale. Allocator capacity is not application-live memory.

With allocation recording enabled, [`Rallocator`][__link4] records allocations, successful
resizes, and frees. A resize is one event with the previous and new addresses
and requested sizes. In-place size changes also produce this event.
Failed resizes and calls that keep both the address and size produce no event.
For a move, recording completes before the old block becomes reusable.
A resize keeps one view-local lifetime in Seismograph instead of adding a new allocation.

```rust
rallocator::rallocator!(backend = mimalloc);
```


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/rallocator">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjNhdIQborR2_k_xJd4bTcf2krrNPIcbP72Pw1UdRjkbim_eMDe2BBthYvRhcoQb91LeXjezy1Uboll5llgYG2Qbg7r4p1N8h5sbf1cKAsy4FSNhZIGCanJhbGxvY2F0b3JlMC40LjA
 [__link0]: https://github.com/microsoft/snmalloc
 [__link1]: https://github.com/microsoft/mimalloc
 [__link2]: https://docs.rs/seismograph/latest/seismograph/fn.recorder.html
 [__link3]: https://docs.rs/rallocator/0.4.0/rallocator/macro.rallocator.html
 [__link4]: https://docs.rs/rallocator/0.4.0/rallocator/?search=Rallocator
