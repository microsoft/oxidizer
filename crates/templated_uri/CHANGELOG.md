# Changelog

## Unreleased

- ⚠️ Breaking

  - Broad API revisit with many breaking changes across `Uri`, `BaseUri`, `UriPath`, `Origin`, and related types. Notable renames include `TargetPathAndQuery` → `UriPath`, `TemplatedPathAndQuery` → `UriTemplate`, `ValidationError` → `UriError`, and `Uri::base_uri(...)` / `Uri::path_and_query(...)` setters → `Uri::with_base(...)` / `Uri::with_path(...)`. Several constructors and conversion helpers were removed in favor of standard `From`/`TryFrom` impls and `from_static` / `from_parts` constructors. Review call sites against the updated API surface.
  - Removed the `http` re-export module. The `Authority`, `PathAndQuery`, and `Scheme` types from `http::uri` are now re-exported directly at the crate root (e.g. `templated_uri::Scheme` instead of `templated_uri::http::Scheme`). `Parts` is no longer re-exported.
  - Renamed `UriSafe<T>` → `UriEscaped<T>`, `UriSafeString` → `UriEscapedString`, `UriSafeError` → `UriEscapeError`, `UriParam::as_uri_safe()` → `UriParam::as_uri_escaped()`, and `UriEscapedString::encode()` → `UriEscapedString::escape()`. The "escaped" wording better reflects that the wrapper proves the value is already percent-encoded (or otherwise composed only of RFC 6570 unreserved characters), not that the URI itself is safe to visit.
  - Renamed `UriPath` → `Path`, `UriTemplate` → `PathTemplate`, `UriEscaped<T>` → `Escaped<T>`, `UriEscapedString` → `EscapedString`, `UriEscapeError` → `EscapeError`, `UriParam` → `Escape` (both the trait and the derive macro), `UriUnsafeParam` → `UnescapedDisplay` (both the trait and the derive macro), the `UriParam::as_uri_escaped()` method → `Escape::escape()`, and the `UriUnsafeParam::as_display()` method → `UnescapedDisplay::unescaped_display()`. The `Uri` prefix was redundant given the crate name; `Path`/`PathTemplate` continue to represent both the path and the optional query string portion of a URI.
  - Refined the `PathTemplate` trait. Implementations no longer derive `Display`; instead, the trait now exposes a `render()` method (returning `String`) for plain rendering. The trait still requires `RedactedDisplay` and `Debug`. `to_uri_string()` was removed in favor of `render()` (or `to_string()` on the parent `Path`/`Uri`). `to_http_path()` was renamed to `to_path_and_query()`, and `into_uri()` was removed (the blanket `From<T: PathTemplate> for Uri` impl already covers this via `Uri::from(...)` / `.into()`). The previous `template()` (Rust format string) was renamed to `format_template()`, and the previous `rfc_6570_template()` (canonical RFC 6570 string) is now `template()`.
  - `Path` now mirrors `Uri`: it implements `RedactedDisplay` and `RedactedDebug`, and shadows `ToString::to_string()` to return `Sensitive<String>` classified under `Uri::DATA_CLASS`. The previous `Path::to_uri_string()` and `Path::to_uri_string_redacted()` were removed (use `to_string()` and the `RedactedToString` trait instead). Renamed `Uri::to_http_path()` → `Uri::to_path_and_query()`.

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
