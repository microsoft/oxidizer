# Stabilization Notes

These notes describe the stable boundary shared by the `thread_aware` crates.

## Stable boundary

`thread_aware_core` 1.0 contains the `no_std` API that downstream crates may
expose:

- `ThreadAware`
- `Affinity`

The `thread_aware` crate re-exports both types so existing imports continue to
work. In particular, `thread_aware::ThreadAware` and
`thread_aware::affinity::Affinity` remain the preferred paths for users of the
full library.

The core crate has no Cargo features or dependencies. It provides
implementations only for types available from `core` and `alloc`. Implementations
for types from external crates are intentionally outside the stable boundary.

## Unstable utilities

Implementation helpers, containers, callbacks, registry APIs, derive support,
and integration helpers remain in the pre-1.0 `thread_aware` crate. Stable
downstream crates should not expose those types in their public APIs.

This split allows the trait contract and its required affinity identifier to
remain stable without prematurely stabilizing the larger utility surface.
