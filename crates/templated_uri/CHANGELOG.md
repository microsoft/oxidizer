# Changelog

## [0.2.4] - 2026-06-04

- 🔧 Maintenance

  - bump `ohno_macros` to 0.3.3


- ✨ Features

  - add serde Deserialize for Uri and PathAndQuery ([#473](https://github.com/microsoft/oxidizer/pull/473))

## [0.2.3] - 2026-06-02

- 🔧 Maintenance

  - Now requires `0.11.2` of `data_privacy`
  - Now requires `0.9.2` of `data_privacy_macros`
  - Now requires `0.9.2` of `data_privacy_macros_impl`
  - Now requires `0.3.4` of `ohno`
  - Now requires `0.3.2` of `ohno_macros`
  - Now requires `0.2.2` of `templated_uri_macros`
  - Now requires `0.2.2` of `templated_uri_macros_impl`

- ✨ Features

  - add `TryFrom<&str>` for `PathAndQuery` ([#464](https://github.com/microsoft/oxidizer/pull/464))
  - release all packages for MSRV increment ([#463](https://github.com/microsoft/oxidizer/pull/463))
  - add try_effective_port method ([#451](https://github.com/microsoft/oxidizer/pull/451))
  - add effective_port and make port return explicit port ([#438](https://github.com/microsoft/oxidizer/pull/438))

- ✔️ Tasks

  - Tidy cargo dependencies to unbreak publishing ([#466](https://github.com/microsoft/oxidizer/pull/466))
  - bump MSRV to 1.91 and refresh dependencies ([#457](https://github.com/microsoft/oxidizer/pull/457))
  - bump templated_uri version ([#444](https://github.com/microsoft/oxidizer/pull/444))

## [0.2.2] - 2026-06-01

- 🔧 Maintenance

  - Now requires `0.11.1` of `data_privacy`
  - Now requires `0.9.1` of `data_privacy_macros`
  - Now requires `0.9.1` of `data_privacy_macros_impl`
  - Now requires `0.3.3` of `ohno`
  - Now requires `0.3.1` of `ohno_macros`
  - Now requires `0.2.1` of `templated_uri_macros`
  - Now requires `0.2.1` of `templated_uri_macros_impl`

- ✨ Features

  - add effective_port and make port return explicit port ([#438](https://github.com/microsoft/oxidizer/pull/438))

- ✔️ Tasks

  - bump MSRV to 1.91 and refresh dependencies ([#457](https://github.com/microsoft/oxidizer/pull/457))
  - bump templated_uri version ([#444](https://github.com/microsoft/oxidizer/pull/444))

## [0.2.1] - 2026-05-25

- ✨ Features

  - add effective_port and make port return explicit port ([#438](https://github.com/microsoft/oxidizer/pull/438))

## [0.2.0] - 2026-05-11

- ✨ Features

  - Support `Option<T>` fields in `#[templated]` structs for RFC 6570 undefined variable semantics. When a field is `None`, it is omitted from the rendered URI, including any associated prefix or separator. ([#408](https://github.com/microsoft/oxidizer/pull/408))

- ⚠️ Breaking

    API review and overall cleanup ([#391](https://github.com/microsoft/oxidizer/pull/391)). Many breaking changes across the crate. 
    Review call sites against the updated API surface. Highlights:

    - Dropped the `Uri` prefix from most types (e.g. `UriPath` → `PathAndQuery`, `UriTemplate` → `PathAndQueryTemplate`, `UriSafe*` → `Escaped*`, `UriParam` → `Escape`, `UriUnsafeParam` → `Raw`), and renamed several other types and methods for consistency (e.g. `ValidationError` → `UriError`, `Uri::to_http_path()` → `Uri::to_path_and_query()`).
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
