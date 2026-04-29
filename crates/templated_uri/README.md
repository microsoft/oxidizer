<div align="center">
 <img src="./logo.png" alt="Templated Uri Logo" width="96">

# Templated Uri

[![crate.io](https://img.shields.io/crates/v/templated_uri.svg)](https://crates.io/crates/templated_uri)
[![docs.rs](https://docs.rs/templated_uri/badge.svg)](https://docs.rs/templated_uri)
[![MSRV](https://img.shields.io/crates/msrv/templated_uri)](https://crates.io/crates/templated_uri)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](../../LICENSE)
<a href="../.."><img src="../../logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Standards-compliant URI handling with templating, validation, and data classification.

This crate provides comprehensive URI manipulation capabilities designed for HTTP clients
and servers that need type-safe, efficient, and data classification-aware URI handling. It builds
on top of the standard `http` crate while adding additional validation guarantees, templating
capabilities, and data classification features.

## Core Types

The crate centers around several key abstractions:

* [`Uri`][__link0] - Flexible URI type composed of an optional [`BaseUri`][__link1] and an optional path/query
* [`BaseUri`][__link2] - Lightweight type representing scheme, authority, and optional base path ([`BasePath`][__link3])
* [`PathAndQueryTemplate`][__link4] - RFC 6570 Level 3 compliant URI templating
* [`Escaped`][__link5] and [`EscapedString`][__link6] - Generic newtype wrapper proving a value is properly escaped for URI components
  by not containing any reserved characters

## Basic Usage

### Simple URI Construction

```rust
use templated_uri::{BaseUri, Uri, PathAndQuery};

// Create the base (scheme + authority, optionally a path prefix)
let base_uri = BaseUri::from_static("https://api.example.com");

// Create a path (can be static for zero-allocation)
let path: PathAndQuery = PathAndQuery::from_static("/api/v1/users");

// Combine into complete URI
let uri = Uri::default().with_base(base_uri).with_path_and_query(path);
assert_eq!(
    uri.to_string().declassify_ref(),
    "https://api.example.com/api/v1/users"
);
```

### Templated URIs

For dynamic URIs with variable components, use the templating system:

```rust
use templated_uri::{BaseUri, PathAndQueryTemplate, Uri, EscapedString, templated};

#[templated(template = "/users/{user_id}/posts/{post_id}", unredacted)]
#[derive(Clone)]
struct UserPostPath {
    user_id: u32,
    post_id: EscapedString,
}

let path = UserPostPath {
    user_id: 42,
    post_id: EscapedString::escape("my-post"),
};

let uri = Uri::default()
    .with_base(BaseUri::from_static("https://api.example.com"))
    .with_path_and_query(path);
```

## URI Escaping Guarantees

The [`Escaped<T>`][__link7] newtype wraps values that are guaranteed
to contain only valid URI characters. This prevents common URI injection vulnerabilities:

```rust
use templated_uri::EscapedString;

// This will succeed - percent-encodes any invalid characters
let encoded = EscapedString::escape("hello world?foo=bar");
assert_eq!(encoded.as_str(), "hello%20world%3Ffoo%3Dbar");

// This will succeed - contains only valid characters
let valid = EscapedString::try_new("hello-world_123").unwrap();
assert_eq!(valid.as_str(), "hello-world_123");

// try_new() fails on URI-reserved characters
let invalid = EscapedString::try_new("hello world?foo=bar");
assert!(invalid.is_err());
```

Built-in valid types include numeric types (`u32`, `u64`, etc.), `Uuid` (with the `uuid` feature),
IP addresses, and validated [`EscapedString`][__link8] instances.

## Telemetry Labels

For complex templates, use the `label` attribute to provide a concise identifier
for telemetry. When present, the label takes precedence over the template string.

```rust
use templated_uri::{EscapedString, templated};

#[templated(
    template = "/{org}/users/{user_id}/reports/{report_type}",
    label = "user_report",
    unredacted
)]
struct ReportPath {
    org: EscapedString,
    user_id: EscapedString,
    report_type: EscapedString,
}
```

## Data Classification

The crate integrates with `data_privacy` to track data sensitivity levels
in URIs. This is particularly important for compliance and data security:

```rust
use data_privacy::Sensitive;
use templated_uri::{EscapedString, templated};

#[templated(template = "/{org_id}/user/{user_id}/")]
#[derive(Clone)]
struct UserPath {
    #[unredacted]
    org_id: EscapedString,
    user_id: Sensitive<EscapedString>,
}
```

## RFC 6570 Template Compliance

The templating system implements [RFC 6570][__link9]
Level 3 URI Template specification. Supported expansions include:

* Simple string expansion: `{var}`
* Reserved string expansion: `{+var}`
* Path segments: `{/var}`
* Query parameters: `{?var}`
* Query continuation: `{&var}`

Note: Fragment expansion (`{#var}`) from RFC 6570 is **not supported** because URI
fragments are stripped by the `http` crate and ignored by HTTP clients.

Template variables must implement [`Escape`][__link10] (except for reserved expansions,
which use [`Raw`][__link11]) to ensure the resulting URI is valid.

## Integration with HTTP Ecosystem

This crate seamlessly integrates with the broader Rust HTTP ecosystem by re-exporting
and building upon the standard [`http`][__link12] crate types. The resulting [`Uri`][__link13] can be converted
to an [`http::Uri`][__link14] for use with HTTP clients
and servers based on [`hyper`][__link15] like [`reqwest`][__link16].


<hr/>
<sub>
This crate was developed as part of <a href="../..">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/templated_uri">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGkYW0CYXSEGy4k8ldDFPOhG2VNeXtD5nnKG6EPY6OfW5wBG8g18NOFNdxpYXKEGxHx8QfjNPcpGxCmQHogYME4G48WgVKUDCN4G-N8o9dvp3-sYWSCgmRodHRwZTEuNC4wgm10ZW1wbGF0ZWRfdXJpZTAuMS4y
 [__link0]: https://docs.rs/templated_uri/0.1.2/templated_uri/?search=Uri
 [__link1]: https://docs.rs/templated_uri/0.1.2/templated_uri/?search=BaseUri
 [__link10]: https://docs.rs/templated_uri/0.1.2/templated_uri/?search=Escape
 [__link11]: https://docs.rs/templated_uri/0.1.2/templated_uri/?search=Raw
 [__link12]: https://docs.rs/http/latest/http/
 [__link13]: https://docs.rs/templated_uri/0.1.2/templated_uri/?search=Uri
 [__link14]: https://docs.rs/http/1.4.0/http/?search=Uri
 [__link15]: https://docs.rs/hyper/latest/hyper/
 [__link16]: https://docs.rs/reqwest/latest/reqwest/
 [__link2]: https://docs.rs/templated_uri/0.1.2/templated_uri/?search=BaseUri
 [__link3]: https://docs.rs/templated_uri/0.1.2/templated_uri/?search=BasePath
 [__link4]: https://docs.rs/templated_uri/0.1.2/templated_uri/?search=PathAndQueryTemplate
 [__link5]: https://docs.rs/templated_uri/0.1.2/templated_uri/?search=Escaped
 [__link6]: https://docs.rs/templated_uri/0.1.2/templated_uri/?search=EscapedString
 [__link7]: https://docs.rs/templated_uri/0.1.2/templated_uri/?search=Escaped
 [__link8]: https://docs.rs/templated_uri/0.1.2/templated_uri/?search=EscapedString
 [__link9]: https://datatracker.ietf.org/doc/html/rfc6570
