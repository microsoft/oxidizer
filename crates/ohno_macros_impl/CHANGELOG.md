# Changelog

## [Unreleased]

- 🔧 Maintenance

  - Initial release of `ohno_macros_impl`, holding the implementation of the `ohno`
    procedural macros. `ohno_macros` is now a thin `proc-macro` shim that delegates
    here. Use the re-exports from `ohno` rather than depending on this crate directly.
