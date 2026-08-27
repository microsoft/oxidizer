# Changelog

## [0.5.1] - 2026-08-27

- 🔧 Maintenance

  - Initial release of `ohno_macros_impl`, holding the implementation of the `ohno`
    procedural macros. `ohno_macros` is now a thin `proc-macro` shim that delegates
    here. Use the re-exports from `ohno` rather than depending on this crate directly.

- ♻️ Code Refactoring

  - split the implementation into observed_macros_impl ([#686](https://github.com/microsoft/oxidizer/pull/686))
