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

## Why use it

The design targets allocation-heavy multithreaded and thread-per-core
workloads: keep ordinary allocations local and amortize cross-thread frees.
Built-in Seismograph integration can record allocation events and expose
allocator state for diagnosing memory use. Enable recording through
[`seismograph::recorder()`][__link0]
when diagnostics are needed. Every backend records allocation events, including the system allocator.
Only snmalloc and mimalloc provide allocator state snapshots.

## Usage

Add the dependency:

```toml
[dependencies]
rallocator = "0.4"
```

Call [`rallocator!`][__link1] once in your executable:

```rust
rallocator::rallocator!(); // Uses the system allocator by default.
```

|Personality|Allocator|
|-----------|---------|
|`rallocator!(backend = system)`|System allocator (default)|
|`rallocator!(backend = snmalloc)`|Rust port of [snmalloc][__link2]|
|`rallocator!(backend = mimalloc)`|Experimental Rust port of [mimalloc][__link3]|
|`rallocator!(backend = dynamic)`|Selected at startup|

Only `dynamic` reads the variable below. Set it before launch. The first allocation fixes the choice for the process.
The optional `default` names the allocator for an unset, empty, or unknown value, for example
`rallocator!(backend = dynamic, default = snmalloc)`. Use `system`, `snmalloc`, or `mimalloc`.
If you omit `default`, `dynamic` uses `system`.

|`RALLOCATOR_DYNAMIC_PERSONALITY`|Allocator used|
|--------------------------------|--------------|
|`snmalloc`|Rust snmalloc|
|`mimalloc`|Rust mimalloc|
|`system`|System allocator|
|Unset or empty|The `default` allocator|
|Any other value|The `default` allocator, after a warning on standard error|


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/rallocator">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjNhdIQborR2_k_xJd4bTcf2krrNPIcbP72Pw1UdRjkbim_eMDe2BBthYvRhcoQbQ6-wcnyIDM8b6SDn8WAIpfIbGfeY9xXcLV8btSsZXnhR-mFhZIGCanJhbGxvY2F0b3JlMC41LjA
 [__link0]: https://docs.rs/seismograph/latest/seismograph/fn.recorder.html
 [__link1]: https://docs.rs/rallocator/0.5.0/rallocator/macro.rallocator.html
 [__link2]: https://github.com/microsoft/snmalloc
 [__link3]: https://github.com/microsoft/mimalloc
