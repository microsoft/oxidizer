<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Thread Aware Core Logo" width="96">

# Thread Aware Core

[![crate.io](https://img.shields.io/crates/v/thread_aware_core.svg)](https://crates.io/crates/thread_aware_core)
[![docs.rs](https://docs.rs/thread_aware_core/badge.svg)](https://docs.rs/thread_aware_core)
[![MSRV](https://img.shields.io/crates/msrv/thread_aware_core)](https://crates.io/crates/thread_aware_core)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Stable foundations for moving thread-isolated state between execution contexts.

This crate contains the small API shared by thread-aware libraries:

* [`ThreadAware`][__link0] notifies a value that it has moved to a different affinity.
* [`Affinity`][__link1] identifies the processor and memory region associated with an
  execution context.

Relocation is a cooperative performance optimization rather than a correctness
boundary. Implementations must remain correct if a relocation notification is
omitted, repeated, or reports the same source and destination.

## Cargo features

The default `std` feature adds implementations for standard-library types such
as `HashMap` and `Path`. Disable default features to use the core API with only
`core` and `alloc`.

Optional `bytes`, `http`, `jiff02`, and `uuid` features add implementations for
selected inert types from those crates. No third-party dependencies are required
unless one of these integration features is enabled.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/thread_aware_core">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjJhdIQb11VxC_uAPOQbtUn4Wx2-BfAbid3Nt1Y27Pobprn8Z6FjFy9hYvRhcoQbShX8a_r6gnUbZjOUFwoF8ccbRmXOuaJStpwbKe0MhKW5cv5hZIGCcXRocmVhZF9hd2FyZV9jb3JlZTEuMC4w
 [__link0]: https://docs.rs/thread_aware_core/1.0.0/thread_aware_core/trait.ThreadAware.html
 [__link1]: https://docs.rs/thread_aware_core/1.0.0/thread_aware_core/struct.Affinity.html
