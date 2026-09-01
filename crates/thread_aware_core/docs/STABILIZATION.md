# Stabilization Notes

These notes describe the stable boundary shared by the `thread_aware` crates.

## Status

`thread_aware_core` is a stand-alone vocabulary crate with no workspace
dependents. The `thread_aware` crate separately exposes its `Affinity`-based
relocation API and utility surface. The two crates therefore have independent
public contracts.

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

Runtime integration code constructs these identifiers through the doc-hidden,
versioned `__private::v1::{new_thread, new_owner, new_numa_node}` functions. The
inherent constructors are crate-private, keeping construction plumbing out of
the stable surface that downstream libraries expose.

Nothing the crate depends on reaches a consumer build; its only manifest entry
is a test-only dev-dependency. The `std` feature is enabled by default and adds
implementations for standard-library types such as `HashMap`, `Path` and
`PathBuf`. Turn it off for `no_std`, where the crate needs only `alloc` and
pointer-width atomics; a
`Thread` then loses its thread id component and cannot be constructed, leaving `Owner` and
`NumaNode` readable so that a `no_std` library can still implement
`ThreadAware`. Implementations for types from external crates are intentionally
outside the stable boundary.

## Unstable utilities

Implementation helpers, containers, callbacks, registry APIs, derive support,
and integration helpers remain in the pre-1.0 `thread_aware` crate. Stable
downstream crates should not expose those types in their public APIs.

This split allows the trait contract and its required `Thread` type to remain
stable without prematurely stabilizing the larger utility surface.
