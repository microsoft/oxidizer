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

The core crate has no required third-party dependencies. Its optional `bytes`,
`http`, `jiff02`, and `uuid` features provide implementations for foreign types.
These implementations must live with the trait because Rust's coherence rules
prevent the higher-level `thread_aware` crate from implementing a foreign trait
for foreign types. The `thread_aware` features of the same names forward to these
core features.

## Unstable utilities

Implementation helpers, containers, callbacks, registry APIs, derive support,
and integration helpers remain in the pre-1.0 `thread_aware` crate. Stable
downstream crates should not expose those types in their public APIs. The
feature-gated foreign-type implementations in `thread_aware_core` add trait
implementations only; they do not add new public types to the stable core API.

This split allows the trait contract and its required affinity identifier to
remain stable without prematurely stabilizing the larger utility surface.
