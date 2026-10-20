# Changelog

## [0.2.2] - 2026-10-20

### Release assessment

- Includes accumulated released-content changes since 0.2.1.

## [0.2.1] - 2026-10-15

- Features

  - Parse `fakes_feature = false` and emit `cfg(test)` gates for fake storage, constructors, delegation, and generated mocks ([#823](https://github.com/microsoft/oxidizer/pull/823)).
  - Generate Mockall methods with `pub(crate)` visibility, including signatures that use crate-private types ([#823](https://github.com/microsoft/oxidizer/pull/823)).
  - Parse explicit `mockall_fake_visibility` for named mock modules and current-module mock types ([#823](https://github.com/microsoft/oxidizer/pull/823)).

- Bug Fixes

  - Add an impl-wide `clippy::unused_self` allowance before original real-impl attributes so explicit consumer lint settings retain precedence ([#823](https://github.com/microsoft/oxidizer/pull/823)).
