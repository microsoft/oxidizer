# Changelog

## [0.6.0] - 2026-10-29

- 🔧 Maintenance

  - Now requires `0.6.0` of `arty_macros_impl`

- ✨ Features

  - add panic policy configuration ([#898](https://github.com/microsoft/oxidizer/pull/898))
  - add monitor configuration plumbing ([#885](https://github.com/microsoft/oxidizer/pull/885))

- 🐛 Bug Fixes

  - default to one worker and relocate spawn results ([#875](https://github.com/microsoft/oxidizer/pull/875))
  - honor exact worker counts ([#873](https://github.com/microsoft/oxidizer/pull/873))

- ♻️ Code Refactoring

  - unify task completion and shutdown ([#899](https://github.com/microsoft/oxidizer/pull/899))

## [0.5.0] - 2026-10-20

- Align the procedural macro package with the Arty 0.5 release.
- Update local dependency requirements: arty_macros_impl =0.4.0 -> 0.5.0.
