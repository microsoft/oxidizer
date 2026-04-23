# Changelog

## Unreleased

- ⚠️ Breaking

  - Broad API revisit with many breaking changes across `Uri`, `BaseUri`, `UriPath`, `Origin`, and related types. Notable renames include `TargetPathAndQuery` → `UriPath`, `TemplatedPathAndQuery` → `UriTemplate`, `ValidationError` → `UriError`, and `Uri::base_uri(...)` / `Uri::path_and_query(...)` setters → `Uri::with_base(...)` / `Uri::with_path(...)`. Several constructors and conversion helpers were removed in favor of standard `From`/`TryFrom` impls and `from_static` / `from_parts` constructors. Review call sites against the updated API surface.

## [0.1.2] - 2026-04-16

- ✨ Features

  - add support for `ErrorLabel` and bump `ohno` version

## [0.1.1] - 2026-04-10

- ✨ Features

  - Support redaction suppression. ([#332](https://github.com/microsoft/oxidizer/pull/332))

- 🐛 Bug Fixes

  - restore const on UriSafeString::from_static ([#328](https://github.com/microsoft/oxidizer/pull/328))

- 📚 Documentation

  - fix BaseUri docs to reflect path prefix support ([#327](https://github.com/microsoft/oxidizer/pull/327))

- ♻️ Code Refactoring

  - use re-exported macros instead of importing templated_uri_macros directly ([#324](https://github.com/microsoft/oxidizer/pull/324))
