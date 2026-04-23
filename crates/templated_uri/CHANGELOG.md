# Changelog

## Unreleased

- ✨ Features

  - Add `Uri::from_parts(base, path)` constructor.

- ⚠️ Breaking

  - Rename `Uri::base_uri(...)` setter to `Uri::with_base(...)` and `Uri::path_and_query(...)` setter to `Uri::with_path(...)`.
  - Remove `Uri::with_base_and_path(Option<BaseUri>, Option<UriPath>)` — use `Uri::default()` with chained setters or `Uri::from_parts(base, path)` instead.
  - Remove `Uri::to_http_uri` and `Uri::into_http_uri` — use `http::Uri::try_from(uri)` / `uri.try_into()` (the `TryFrom<Uri> for http::Uri` impl) instead.
  - Rename `UriPath::from_path_and_query` to `UriPath::from_http_path`.
  - Rename `BaseUri::from_uri_static` to `BaseUri::from_static`.
  - Remove `BaseUri::from_uri_str` — use the `FromStr` impl (`s.parse::<BaseUri>()`) or new `TryFrom<&str>` impl.
  - Rename `BaseUri::from_http_uri(&http::Uri)` to `BaseUri::from_http(&http::Uri)`; also added `TryFrom<&http::Uri> for BaseUri`.
  - Rename `Uri::to_path_and_query` to `Uri::to_http_path`.
  - Rename `UriPath::to_path_and_query` to `UriPath::to_http_path`.
  - Rename `UriTemplate::to_path_and_query` to `UriTemplate::to_http_path`.
  - Rename `Uri::target_path_and_query` to `Uri::to_path` (now returns owned `Option<UriPath>`).
  - Rename `TargetPathAndQuery` to `UriPath` and hide its enum variants behind a transparent struct.
  - Rename `TemplatedPathAndQuery` trait to `UriTemplate`.
  - Rename `ValidationError` to `UriError`.
  - Replace `DATA_CLASS_UNKNOWN_URI` constant with `Uri::DATA_CLASS` associated constant.
  - Rename `UriPath::from_templated` to `UriPath::from_template`.
  - Rename `Origin::new(scheme, authority)` to `Origin::from_parts(scheme, authority)`.
  - Remove `BaseUri::new(scheme, authority)` — use `Origin::from_parts(scheme, authority)?.into()` (via `From<Origin> for BaseUri`).
  - Replace `BaseUri::from_parts(scheme, host, port, path)` with infallible `BaseUri::from_parts(origin: impl Into<Origin>, path: impl Into<BasePath>) -> Self`.
  - Add fallible `BaseUri::from_host_and_port(scheme, host, port, path)` convenience constructor (covers the previous 4-arg `from_parts` use case).
  - Make `Uri::to_http_path`, `UriPath::to_http_path`, and `UriTemplate::to_http_path` infallible. `Uri::to_http_path` now returns `Option<PathAndQuery>`; the trait method returns `PathAndQuery` directly. The proc-macro-generated impl panics if a template parameter produces a non-URI-safe value, which should never happen with `UriSafeString`-backed parameters.

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
