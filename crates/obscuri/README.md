<div align="center">
 <img src="./logo.png" alt="Obscuri Logo" width="96">

# Obscuri

[![crate.io](https://img.shields.io/crates/v/obscuri.svg)](https://crates.io/crates/obscuri)
[![docs.rs](https://docs.rs/obscuri/badge.svg)](https://docs.rs/obscuri)
[![MSRV](https://img.shields.io/crates/msrv/obscuri)](https://crates.io/crates/obscuri)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](../../LICENSE)
<a href="../.."><img src="../../logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Standards-compliant URI handling with templating, safety validation, and data classification.

This crate provides comprehensive URI manipulation capabilities designed for HTTP clients
and servers that need type-safe, efficient, and data classification-aware URI handling. It builds
on top of the standard `http` crate while adding additional safety guarantees, templating
capabilities, and data classification features.

## Core Types

The crate centers around several key abstractions:

* [`Uri`][__link0] - Flexible URI type with endpoint and path/query components
* [`BaseUri`][__link1] - Lightweight type representing scheme and authority (no path/query)
* [`TemplatedPathAndQuery`][__link2] - RFC 6570 Level 3 compliant URI templating
* [`UriSafe`][__link3] and [`UriSafeString`][__link4] - Trait and its implementation for String, marking type as safe for URI components
  by not containing any reserved characters

## Basic Usage

### Simple URI Construction

```rust
use obscuri::uri::{PathAndQuery, TargetPathAndQuery};
use obscuri::{BaseUri, Uri};

// Create an endpoint (scheme + authority only)
let base_uri = BaseUri::from_uri_static("https://api.example.com");

// Create a path (can be static for zero-allocation)
let path: TargetPathAndQuery = TargetPathAndQuery::from_static("/api/v1/users");

// Combine into complete URI
let uri = Uri::default().base_uri(base_uri).path_and_query(path);
assert_eq!(
    uri.to_string().declassify_ref(),
    "https://api.example.com/api/v1/users"
);
```

### Templated URIs

For dynamic URIs with variable components, use the templating system:

```rust
use obscuri::{BaseUri, TemplatedPathAndQuery, Uri, UriSafeString, templated};
use uuid::Uuid;

#[templated(template = "/users/{user_id}/posts/{post_id}", unredacted)]
#[derive(Clone)]
struct UserPostPath {
    user_id: Uuid,
    post_id: UriSafeString,
}

let path = UserPostPath {
    user_id: Uuid::new_v4(),
    post_id: UriSafeString::new(&"my-post").unwrap(),
};

let uri = Uri::default()
    .base_uri(BaseUri::from_uri_static("https://api.example.com"))
    .path_and_query(path);
```

## URI Safety Guarantees

The crate provides [`UriSafe`][__link5] trait implementations for types that are guaranteed
to contain only URI-safe characters. This prevents common URI injection vulnerabilities:

```rust
use obscuri::UriSafeString;

// This will succeed - contains only safe characters
let safe = UriSafeString::new(&"hello-world_123").unwrap();

// This will fail - contains URI-reserved characters
let unsafe_string = UriSafeString::new(&"hello world?foo=bar");
assert!(unsafe_string.is_err());
```

Built-in safe types include numeric types (`u32`, `u64`, etc.), [`Uuid`][__link6],
IP addresses, and validated [`UriSafeString`][__link7] instances.

## Telemetry Labels

For complex templates, use the `label` attribute to provide a concise identifier
for telemetry. When present, the label takes precedence over the template string.

```rust
use obscuri::{UriSafeString, templated};

#[templated(
    template = "/{org}/users/{user_id}/reports/{report_type}",
    label = "user_report",
    unredacted
)]
struct ReportPath {
    org: UriSafeString,
    user_id: UriSafeString,
    report_type: UriSafeString,
}
```

## Data Classification

The crate integrates with `data_privacy` to track data sensitivity levels
in URIs. This is particularly important for compliance and data security:

```rust
use data_privacy::Sensitive;
use obscuri::{UriSafeString, templated};

#[templated(template = "/{org_id}/user/{user_id}/")]
#[derive(Clone)]
struct UserPath {
    #[unredacted]
    org_id: UriSafeString,
    user_id: Sensitive<UriSafeString>,
}
```

## RFC 6570 Template Compliance

The templating system implements [RFC 6570][__link8]
Level 3 URI Template specification. Supported expansions include:

* Simple string expansion: `{var}`
* Reserved string expansion: `{+var}`
* Fragment expansion: `{#var}`
* Path segments: `{/var}`
* Query parameters: `{?var}`
* Query continuation: `{&var}`

Template variables must implement [`UriSafe`][__link9] (except for fragment and reserved expansions)
to ensure the resulting URI is valid.

## Integration with HTTP Ecosystem

This crate seamlessly integrates with the broader Rust HTTP ecosystem by re-exporting
and building upon the standard [`http`][__link10] crate types. The resulting [`Uri`][__link11] can be converted
to an [`http::Uri`][__link12] for use with HTTP clients
and servers based on [`hyper`][__link13] like [`reqwest`][__link14].


<hr/>
<sub>
This crate was developed as part of <a href="../..">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/obscuri">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGkYW0CYXSEGy4k8ldDFPOhG2VNeXtD5nnKG6EPY6OfW5wBG8g18NOFNdxpYXKEG7z17Wpz_pFOG3Sgtc2zhgIWG_Zo5SnfNLaEG81kh3w2WQhmYWSDgmRodHRwZTEuNC4wgmdvYnNjdXJpZTAuMS4wgmR1dWlkZjEuMjEuMA
 [__link0]: https://docs.rs/obscuri/0.1.0/obscuri/?search=uri::Uri
 [__link1]: https://docs.rs/obscuri/0.1.0/obscuri/?search=BaseUri
 [__link10]: https://docs.rs/http/latest/http/
 [__link11]: https://docs.rs/obscuri/0.1.0/obscuri/?search=uri::Uri
 [__link12]: https://docs.rs/http/1.4.0/http/?search=Uri
 [__link13]: https://docs.rs/hyper/latest/hyper/
 [__link14]: https://docs.rs/reqwest/latest/reqwest/
 [__link2]: https://docs.rs/obscuri/0.1.0/obscuri/?search=TemplatedPathAndQuery
 [__link3]: https://docs.rs/obscuri/0.1.0/obscuri/?search=UriSafe
 [__link4]: https://docs.rs/obscuri/0.1.0/obscuri/?search=UriSafeString
 [__link5]: https://docs.rs/obscuri/0.1.0/obscuri/?search=UriSafe
 [__link6]: https://docs.rs/uuid/1.21.0/uuid/?search=Uuid
 [__link7]: https://docs.rs/obscuri/0.1.0/obscuri/?search=UriSafeString
 [__link8]: https://datatracker.ietf.org/doc/html/rfc6570
 [__link9]: https://docs.rs/obscuri/0.1.0/obscuri/?search=UriSafe
