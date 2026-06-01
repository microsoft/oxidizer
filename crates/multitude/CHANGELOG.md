# Changelog

## [0.1.1] - 2026-06-01

- 🔧 Maintenance

  - Now requires `0.7.1` of `thread_aware`
  - Now requires `0.7.1` of `thread_aware_macros`

- 🐛 Bug Fixes

  - gate gungraun to linux ([#456](https://github.com/microsoft/oxidizer/pull/456))
  - examples collision ([#455](https://github.com/microsoft/oxidizer/pull/455))
  - tighten allocator safety proofs and docs ([#443](https://github.com/microsoft/oxidizer/pull/443))
  - ensure that `cargo test` passes on a clean checkout ([#441](https://github.com/microsoft/oxidizer/pull/441))

- ⚡ Performance

  - split allocator hot paths from cold refill/oversized… ([#442](https://github.com/microsoft/oxidizer/pull/442))

- ✔️ Tasks

  - bump MSRV to 1.91 and refresh dependencies ([#457](https://github.com/microsoft/oxidizer/pull/457))
  - bump templated_uri version ([#444](https://github.com/microsoft/oxidizer/pull/444))

- ♻️ Code Refactoring

  - consolidate unsafe idioms behind shared helpers ([#447](https://github.com/microsoft/oxidizer/pull/447))

## [0.1.0] - 2026-05-21

- ✨ Features

  - Initial release of the `multitude` arena allocator.
