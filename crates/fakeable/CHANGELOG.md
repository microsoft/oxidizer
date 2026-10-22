# Changelog

## [0.2.2] - 2026-10-20

- Published local dependency requirement(s) will move: fakeable_impl.
- Update local dependency requirements: fakeable_impl =0.2.1 -> 0.2.2.

## [0.2.1] - 2026-10-15

- Features

  - Add `fakes_feature = false` for unit-test-only fakes without a consumer Cargo feature ([#823](https://github.com/microsoft/oxidizer/pull/823)).
  - Support generated Mockall fakes for `pub(crate)` methods, preserving their visibility ([#823](https://github.com/microsoft/oxidizer/pull/823)).
  - Add `mockall_fake_visibility` to control generated mock container visibility without changing the public default ([#823](https://github.com/microsoft/oxidizer/pull/823)).

- Bug Fixes

  - Allow `clippy::unused_self` on generated real impls, including private helpers, while preserving explicit consumer lint attributes ([#823](https://github.com/microsoft/oxidizer/pull/823)).

- Dependencies

  - Require exactly `0.2.1` of `fakeable_impl`.
