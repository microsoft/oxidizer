# Stabilization Notes

These notes define the stable boundary shared by the `thread_aware` crates.

## Status

`thread_aware_core` is the authoritative crate for the core types. The `thread_aware` crate depends on it and re-exports its public types. It also provides derive support, wrappers, and runtime construction. The `performables` crate provides strategy-partitioned shared state.

## Stable boundary

`thread_aware_core` 1.0 contains these types:

- `ThreadAware`
- `Thread`
- `Owner`
- `NumaNode`

A `Thread` describes the runtime, OS thread, and nearest memory for a value. `Owner` identifies the runtime. `NumaNode` identifies the nearest memory region. The OS thread uses `std::thread::ThreadId`, so callers get that type from `std`.

`thread_aware::ThreadBuilder` is the supported public runtime integration API. It owns one runtime identifier and can be cloned during worker setup. It creates thread coordinates with an optional NUMA node.

The doc-hidden `__private::v1` constructors are publicly reachable for integration code. They are versioned, unsupported implementation details and are not part of the stable boundary.

The crate has no normal dependencies. Its only manifest dependency is for tests.

The `std` feature is on by default. It adds implementations for `HashMap`, `Path`, `PathBuf`, `SocketAddr`, `SocketAddrV4`, and `SocketAddrV6`.

Without `std`, the crate needs `alloc` and pointer-width atomics. A `Thread` has no thread id and cannot be constructed. A library can still read `Owner` and `NumaNode` and implement `ThreadAware`.

Implementations for external crate types are outside the stable boundary.

## Unstable utilities

These utilities remain in the pre-1.0 `thread_aware` crate:

- The derive macro
- Closure adapters
- Policy wrappers
- `ThreadBuilder`
- Relocation test helpers

Stable downstream crates must not expose these utility types in public APIs.

Strategy-partitioned `Arc` remains in the separate pre-1.0 `performables` crate.

This split keeps the trait, coordinate types, and built-in implementations stable. The larger utility APIs can continue to change.
