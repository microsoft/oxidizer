# Changelog

## [0.4.6] - 2026-06-04

- 🔧 Maintenance

  - bump `ohno_macros` to 0.3.3

## [0.4.5] - 2026-06-04

- 🔧 Maintenance

  - bump `thread_aware_macros_impl` to 0.7.2

## [0.4.4] - 2026-06-02

- 🔧 Maintenance

  - Now requires `0.3.2` of `layered`

- ✨ Features

  - release all packages for MSRV increment ([#463](https://github.com/microsoft/oxidizer/pull/463))

- 🐛 Bug Fixes

  - ensure that `cargo test` passes on a clean checkout ([#441](https://github.com/microsoft/oxidizer/pull/441))

- ✔️ Tasks

  - Release all packages again to unbreak GitHub publishing (part N+1) ([#467](https://github.com/microsoft/oxidizer/pull/467))
  - Tidy cargo dependencies to unbreak publishing ([#466](https://github.com/microsoft/oxidizer/pull/466))
  - bump MSRV to 1.91 and refresh dependencies ([#457](https://github.com/microsoft/oxidizer/pull/457))
  - bump templated_uri version ([#444](https://github.com/microsoft/oxidizer/pull/444))

- 🧩 Miscellaneous

  - Update tool versions ([#462](https://github.com/microsoft/oxidizer/pull/462))

## [0.4.3] - 2026-06-02

- 🔧 Maintenance

  - Now requires `0.5.2` of `bytesbuf`
  - Now requires `0.11.2` of `data_privacy`
  - Now requires `0.9.2` of `data_privacy_macros`
  - Now requires `0.9.2` of `data_privacy_macros_impl`
  - Now requires `0.3.4` of `ohno`
  - Now requires `0.3.2` of `ohno_macros`
  - Now requires `0.1.4` of `recoverable`
  - Now requires `0.2.3` of `templated_uri`
  - Now requires `0.2.2` of `templated_uri_macros`
  - Now requires `0.2.2` of `templated_uri_macros_impl`
  - Now requires `0.7.2` of `thread_aware`
  - Now requires `0.7.2` of `thread_aware_macros`
  - Now requires `0.7.1` of `thread_aware_macros_impl`
  - Now requires `0.3.2` of `tick`

- ✨ Features

  - release all packages for MSRV increment ([#463](https://github.com/microsoft/oxidizer/pull/463))

- 🐛 Bug Fixes

  - ensure that `cargo test` passes on a clean checkout ([#441](https://github.com/microsoft/oxidizer/pull/441))

- ✔️ Tasks

  - Tidy cargo dependencies to unbreak publishing ([#466](https://github.com/microsoft/oxidizer/pull/466))
  - bump MSRV to 1.91 and refresh dependencies ([#457](https://github.com/microsoft/oxidizer/pull/457))
  - bump templated_uri version ([#444](https://github.com/microsoft/oxidizer/pull/444))

- 🧩 Miscellaneous

  - Update tool versions ([#462](https://github.com/microsoft/oxidizer/pull/462))

## [0.4.2] - 2026-06-01

- 🔧 Maintenance

  - Now requires `0.11.1` of `data_privacy`
  - Now requires `0.9.1` of `data_privacy_macros`
  - Now requires `0.9.1` of `data_privacy_macros_impl`
  - Now requires `0.3.1` of `layered`
  - Now requires `0.3.3` of `ohno`
  - Now requires `0.3.1` of `ohno_macros`
  - Now requires `0.1.3` of `recoverable`
  - Now requires `0.2.2` of `templated_uri`
  - Now requires `0.2.1` of `templated_uri_macros`
  - Now requires `0.2.1` of `templated_uri_macros_impl`
  - Now requires `0.7.1` of `thread_aware`
  - Now requires `0.7.1` of `thread_aware_macros`
  - Now requires `0.3.1` of `tick`

- 🐛 Bug Fixes

  - ensure that `cargo test` passes on a clean checkout ([#441](https://github.com/microsoft/oxidizer/pull/441))

- ✔️ Tasks

  - bump MSRV to 1.91 and refresh dependencies ([#457](https://github.com/microsoft/oxidizer/pull/457))
  - bump templated_uri version ([#444](https://github.com/microsoft/oxidizer/pull/444))

## [0.4.1] - 2026-05-25

- 🔧 Maintenance

  - bump `templated_uri` to 0.2.1

## [0.4.0] - 2026-05-18

- ✨ Features

  - add `routing` module with `Router`, `RouterContext`, and `BaseUriConflict` for resolving the target `BaseUri` of outgoing requests
  - `HttpRequestBuilder::build` now attaches the original templated `templated_uri::Uri` as a request extension, which `Router::resolve_request_uri` uses so repeated in-place re-routings (e.g. fallback retries with `BaseUriConflict::UseRouted`) don't duplicate the base path prefix.

- ⚠️ Breaking

  - Rename `UrlTemplateLabel` to `UriTemplateLabel` and `RequestExt::url_template_label()` / `ExtensionsExt::url_template_label()` to `uri_template_label()`.
  - update HTTP body and extension APIs to `bytesbuf` 0.5, `thread_aware` 0.7, `tick` 0.3, and `templated_uri` 0.2

## [0.3.2] - 2026-04-22

- 🔧 Maintenance

  - bump `tick` to 0.2.2

## [0.3.1] - 2026-04-20

- ✨ Features

  - fine grained error labels ([#382](https://github.com/microsoft/oxidizer/pull/382))

## [0.3.0] - 2026-04-15

- ⚠️ Breaking

  - make RequestHandler super trait of Service ([#365](https://github.com/microsoft/oxidizer/pull/365))
  - `HttpError` now uses `ohno::ErrorLabel` ([#366](https://github.com/microsoft/oxidizer/pull/366))
  - add request/response timeouts and refactor body builder ([#362](https://github.com/microsoft/oxidizer/pull/362))

- ✨ Features

  - add extension methods for http::Extensions ([#356](https://github.com/microsoft/oxidizer/pull/356))

## [0.2.1] - 2026-03-24

- ✨ Features

  - introduce http_extensions crate ([#326](https://github.com/microsoft/oxidizer/pull/326))
