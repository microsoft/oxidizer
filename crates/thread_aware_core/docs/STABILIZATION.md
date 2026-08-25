# Stabilization Notes

These notes describe the stable boundary shared by the `thread_aware` crates.

## Status

`thread_aware_core` is a stand-alone crate. Nothing depends on it yet, and the
`thread_aware` crate still ships its own `Affinity`-based API unchanged. The
plan is to adopt the core crate later; until then the two evolve separately.

## Stable boundary

`thread_aware_core` 1.0 contains the API that downstream crates may expose:

- `ThreadAware`
- `Place`, and its component ids `Origin` and `NumaNode`

A `Place` says where a value runs: which runtime produced it (`Origin`), which
thread it is on, and which memory is closest to that thread (`NumaNode`). The
thread component is `std::thread::ThreadId` rather than an id of our own, so it
is not re-exported; callers take it from `std`.

The crate has no dependencies. Its `std` feature is enabled by default and adds
implementations for standard-library types such as `HashMap`, `Path` and
`PathBuf`. Turn it off for `no_std`, where the crate needs only `alloc`; a
`Place` then loses its thread id and cannot be constructed, leaving `Origin` and
`NumaNode` readable so that a `no_std` library can still implement
`ThreadAware`. Implementations for types from external crates are intentionally
outside the stable boundary.

## Unstable utilities

Implementation helpers, containers, callbacks, registry APIs, derive support,
and integration helpers remain in the pre-1.0 `thread_aware` crate. Stable
downstream crates should not expose those types in their public APIs.

This split allows the trait contract and its required place identifier to remain
stable without prematurely stabilizing the larger utility surface.
