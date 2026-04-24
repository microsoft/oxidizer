# Changelog

## Unreleased

- ⚠️ Breaking

  - Broad API cleanup with many breaking changes across the crate. Review call sites against the updated API surface. Highlights:
    - Dropped the `Uri` prefix from most types (e.g. `UriPath` → `PathAndQuery`, `UriTemplate` → `PathAndQueryTemplate`, `UriSafe*` → `Escaped*`, `UriParam` → `Escape`, `UriUnsafeParam` → `UnescapedDisplay`), and renamed several other types and methods for consistency (e.g. `ValidationError` → `UriError`, `Uri::to_http_path()` → `Uri::to_path_and_query()`).
    - `Origin` (and `BaseUri`) now accept any URI scheme, not just HTTP/HTTPS; `Origin::port()` returns `Option<u16>` and `Origin::try_from_parts` was replaced by the infallible `Origin::from_parts`.
    - `PathAndQuery` now mirrors `Uri` for redaction: it implements `RedactedDisplay`/`RedactedDebug` and `to_string()` returns a `Sensitive<String>`.
    - Refined the `PathAndQueryTemplate` trait: added `render()`, removed `Display`/`to_uri_string()`/`into_uri()`, and swapped the meanings of `template()` and `format_template()`.
    - Renamed `Path` → `PathAndQuery` and `PathTemplate` → `PathAndQueryTemplate`; the crate no longer re-exports `http::uri::PathAndQuery`. Reach for `http::uri::PathAndQuery` (or via `templated_uri::http::uri::PathAndQuery`) when you need the underlying validated http type.
    - Removed the `http` re-export module; `Authority` and `Scheme` are now re-exported at the crate root (`Parts` is no longer re-exported).

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
