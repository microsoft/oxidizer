# Changelog

## Unreleased

- ✨ Features

  - add `routing` module with `Routing`, `RoutingContext`, and `BaseUriConflict` for resolving the target `BaseUri` of outgoing requests
- ⚠️ Breaking

  - Rename `UrlTemplateLabel` to `UriTemplateLabel` and `RequestExt::url_template_label()` / `ExtensionsExt::url_template_label()` to `uri_template_label()`.

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
