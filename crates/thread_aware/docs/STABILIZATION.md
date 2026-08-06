# Stabilization Notes

Tracking item: [O365 Core 7688110](https://o365exchange.visualstudio.com/O365%20Core/_workitems/edit/7688110)

These notes capture the pending decisions for stabilizing `thread_aware`. They
describe proposals under review, not a finalized stable API.

## Proposed stable boundary

- Stabilize the `ThreadAware` trait.
- Stabilize the `Affinity` type needed at `ThreadAware` API boundaries.
- Allow stable downstream crates such as `arty`, `arty_io`, and `tick` to expose
  these core types in their public APIs.

## Deferred surface

Keep implementation helpers, containers, callbacks, registry APIs, derive support,
and other uncertain APIs outside the stable surface for now. Downstream stable
crates must not expose deferred types in their public APIs.

The exact deferred set must be verified against the complete public API before
stabilization.

## Packaging boundary

The packaging decision is still open:

- Keep `ThreadAware` and `Affinity` in `thread_aware`, and move the deferred
  surface to an unstable `thread_aware_utils` crate.
- Alternatively, follow the common split where traits move to a
  `thread_aware_core` crate while the remaining APIs stay in `thread_aware`.

If the `thread_aware_utils` proposal is selected, rename the internal companion
crates to `thread_aware_utils_macros` and
`thread_aware_utils_macro_impl`. These crates are not intended for direct
consumption.

## Pending review

- [ ] Explicitly list and review every API included in the stable surface.
- [ ] Confirm stable downstream crates expose only stable `thread_aware` types.
- [ ] Resolve naming feedback for the trait, affinity concepts, and crate split.
- [ ] Confirm the public shape and semantics of `Affinity`.
- [ ] Decide whether the stable trait belongs in `thread_aware` or a
      `thread_aware_core` crate.
- [ ] Identify all deferred APIs as unstable and document their package location.
- [ ] Confirm the names of any renamed macro crates.
