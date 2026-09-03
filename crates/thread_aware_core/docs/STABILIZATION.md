# Stabilization Notes

These notes describe the stable boundary shared by the `thread_aware` crates.

## Status

`thread_aware_core` is the authoritative vocabulary crate. The `thread_aware`
crate depends on it and re-exports its public types alongside derive support,
wrappers, runtime construction, and strategy-partitioned shared state.

Adoption of the core contract across the package family is tracked in
[oxidizer#719](https://github.com/microsoft/oxidizer/issues/719).

## Stable boundary

`thread_aware_core` 1.0 contains the API that downstream crates may expose:

- `ThreadAware`
- `Thread`, and its component ids `Owner` and `NumaNode`

A `Thread` says where a value runs: which runtime owns it (`Owner`), which
OS thread it is on, and which memory is closest to that thread (`NumaNode`). The
thread component is `std::thread::ThreadId` rather than an id of our own, so it
is not re-exported; callers take it from `std`.

`thread_aware::thread::ThreadBuilder` is the public runtime integration API. It
owns one runtime identifier, is cloneable across worker setup, and constructs
thread coordinates with optional NUMA-node selection. Only that builder uses the
doc-hidden, versioned `__private::v1` constructors.

The crate has no normal dependencies; its only manifest dependency is test-only.
The `std` feature is enabled by default and adds
implementations for standard-library types such as `HashMap`, `Path` and
`PathBuf`. Turn it off for `no_std`, where the crate needs only `alloc` and
pointer-width atomics; a
`Thread` then loses its thread id component and cannot be constructed, leaving `Owner` and
`NumaNode` readable so that a `no_std` library can still implement
`ThreadAware`. Implementations for types from external crates are intentionally
outside the stable boundary.

## Unstable utilities

The derive macro, closure adapters, policy wrappers, `ThreadBuilder`,
strategy-partitioned `Arc` and `Storage`, and relocation test helpers remain in
the pre-1.0 `thread_aware` crate. Stable downstream crates should not expose
those types in their public APIs.

This split allows the trait contract, thread coordinate types, and built-in
implementations to remain stable without prematurely stabilizing the larger
utility surface.
