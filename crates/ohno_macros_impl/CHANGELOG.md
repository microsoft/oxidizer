# Changelog

## [0.5.2] - 2026-09-07

- 🐛 Bug Fixes

  - correct the syntax context of a rewritten unit struct and the `#[automatically_derived]` placements ([#723](https://github.com/microsoft/oxidizer/pull/723))

- 📚 Documentation

  - reconcile the design with the two-crate split ([#698](https://github.com/microsoft/oxidizer/pull/698))

## [0.5.1] - 2026-08-27

- 🔧 Maintenance

  - use shared testing infrastructure ([#686](https://github.com/microsoft/oxidizer/pull/686))

## [0.5.0] - 2026-08-21

- 🔧 Maintenance

  - Initial release of `ohno_macros_impl`, holding the implementation of the `ohno`
    procedural macros. `ohno_macros` is now a thin `proc-macro` shim that delegates
    here. Use the re-exports from `ohno` rather than depending on this crate directly.
