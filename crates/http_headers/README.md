<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Http Headers Logo" width="96">

# Http Headers

[![crate.io](https://img.shields.io/crates/v/http_headers.svg)](https://crates.io/crates/http_headers)
[![docs.rs](https://docs.rs/http_headers/badge.svg)](https://docs.rs/http_headers)
[![MSRV](https://img.shields.io/crates/msrv/http_headers)](https://crates.io/crates/http_headers)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml/badge.svg)](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Efficient and robust HTTP header parsing and creation.

This crate provides:

* Highly optimized parsing of incoming HTTP headers which produce owned or borrowed
  strongly-typed Rust structs. These parsers insulate your code from badly formed
  headers.

* Highly optimized production of headers, ensuring the headers are well-formed.

Header parsing and production are abstracted over their source and destination.
The optional `http` feature integrates with
[`HeaderMap`][__link0]
plus generic [`Request`][__link1]
and [`Response`][__link2]
values from the [`http`][__link3] crate.

## Parsing headers

Headers are parsed from an implementation of the [`source::FieldSource`][__link4] trait. The `http` crate feature
implements this trait for [`HeaderMap`][__link5],
[`Request`][__link6], and
[`Response`][__link7].
Once you have a source, you can choose to parse into borrowed views or owned structs.
Prefer borrowed views when the decoded value does not need to outlive the
source as they are generally faster. Use owned structs when the parsed header
data needs to be retained (such as in a cache).

[`Field::view`][__link8] returns a header’s
borrowed `*View` type, whose lifetime is tied to the source.

```rust
use http::HeaderMap;
use http_headers::Field;
use http_headers::headers::{ContentType, UserAgent};

// create a HeaderMap to show how to read from it
let mut headers = HeaderMap::new();
headers.insert(
    http::header::USER_AGENT,
    http::HeaderValue::from_static("example-client/1.0"),
);
headers.insert(
    http::header::CONTENT_TYPE,
    http::HeaderValue::from_static("application/json; charset=utf-8"),
);

if let Some(agent) = UserAgent::view(&headers)? {
    assert_eq!(agent.as_str()?, "example-client/1.0");
}

if let Some(content_type) = ContentType::view(&headers)? {
    assert_eq!(content_type.type_()?, "application");
    assert_eq!(content_type.subtype()?, "json");
    assert_eq!(
        content_type.parameter("charset")?,
        Some(b"utf-8".as_slice())
    );
}
```

Prefer `view` unless the decoded value must outlive the source. Use
[`Field::owned`][__link9] when you need to retain, move, or independently
store the result:

```rust
use http::HeaderMap;
use http_headers::Field;
use http_headers::headers::UserAgent;

let mut headers = HeaderMap::new();
headers.insert(
    http::header::USER_AGENT,
    http::HeaderValue::from_static("example-client/1.0"),
);

let owned = UserAgent::owned(&headers)?.expect("User-Agent is present");
drop(headers);
assert_eq!(owned.as_bytes(), b"example-client/1.0");
```

Both methods return `Ok(None)` when the header is absent and `Err` when a
present value is malformed.

## Producing headers

You produce headers by populating an implementation of the [`sink::FieldSink`][__link10] trait. The
`http` cargo feature implements this trait for
[`HeaderMap`][__link11],
[`Request`][__link12], and
[`Response`][__link13].

The [`sink::FieldSinkExt`][__link14] trait provides fluent methods that work for any sink:

```rust
use std::time::Duration;

use http::HeaderMap;
use http_headers::headers::{CacheControl, ContentType};
use http_headers::sink::FieldSinkExt;

let mut headers = HeaderMap::new();
headers
    .set_content_type(ContentType::json())?
    .set_content_length(1_024)?
    .set_cache_control(CacheControl::public().max_age(Duration::from_secs(60)))?;
```

An owned value can insert itself when it has already been constructed:

```rust
use http::HeaderMap;
use http_headers::headers::LocationOwned;

let mut headers = HeaderMap::new();
LocationOwned::try_from("/next")?.insert_into(&mut headers)?;
```

A borrowed view can also be forwarded directly to another sink:

```rust
use http::HeaderMap;
use http_headers::Field;
use http_headers::headers::UserAgent;

let mut incoming = HeaderMap::new();
incoming.insert(
    http::header::USER_AGENT,
    http::HeaderValue::from_static("example-client/1.0"),
);

let mut outgoing = HeaderMap::new();
if let Some(agent) = UserAgent::view(&incoming)? {
    agent.insert_into(&mut outgoing)?;
}
```

[`Field::insert`][__link15] is the generic alternative when the descriptor type is
already known. It replaces all existing field lines for that header;
[`Field::remove`][__link16] removes them instead.

```rust
use http::HeaderMap;
use http_headers::Field;
use http_headers::headers::{UserAgent, UserAgentOwned};

let mut headers = HeaderMap::new();
UserAgent::insert(
    &mut headers,
    UserAgentOwned::try_from_static("example-client/1.0")?,
)?;
UserAgent::remove(&mut headers);
```

Repeated field lines remain separate. In particular, `Set-Cookie` values are
never comma-joined:

```rust
use http::HeaderMap;
use http_headers::Field;
use http_headers::headers::{SetCookie, SetCookieOwned};

let mut cookies = SetCookieOwned::new();
cookies.push_str("session=abc; Path=/; HttpOnly")?;
cookies.push_str("theme=dark; Path=/")?;

let mut headers = HeaderMap::new();
SetCookie::insert(&mut headers, cookies)?;
assert_eq!(headers.get_all(http::header::SET_COOKIE).iter().count(), 2);
```

## Serialization

The `serde` cargo feature implements Serde serialization and deserialization for every owned header
struct, [`FieldName`][__link17], [`FieldValue`][__link18], and [`sink::EncodedValues`][__link19].
Headers serialize as an ordered sequence of physical field values and
deserialize through relaxed validation, which includes strict syntax and
the documented interoperability deviations. This preserves round trips for
every owned value produced by the public API:

```rust
use http_headers::headers::UserAgentOwned;

let header = UserAgentOwned::try_from("example-client/1.0")?;
let json = serde_json::to_string(&header)?;
let decoded: UserAgentOwned = serde_json::from_str(&json)?;
assert_eq!(decoded.as_bytes(), header.as_bytes());
```

Repeated lines retain their boundaries:

```rust
use http_headers::headers::SetCookieOwned;

let mut cookies = SetCookieOwned::new();
cookies.push_str("session=abc")?;
cookies.push_str("theme=dark")?;
let json = serde_json::to_string(&cookies)?;
let decoded: SetCookieOwned = serde_json::from_str(&json)?;
assert_eq!(decoded.len(), 2);
```

Serialization is not redaction. Sensitive values include their original
bytes and an explicit sensitivity marker, so serialized data must be
protected like the header value itself:

```rust
use http_headers::{FieldSensitivity, FieldValue};

let secret =
    FieldValue::from_static("credential").with_sensitivity(FieldSensitivity::Sensitive);
let json = serde_json::to_string(&secret)?;
let decoded: FieldValue = serde_json::from_str(&json)?;
assert_eq!(decoded.as_bytes(), b"credential");
assert!(decoded.is_sensitive());
```

## Strict and relaxed reads

[`Field::view`][__link20] and [`Field::owned`][__link21] use strict syntax. Applications that
must accept specific common deviations can request [`DecodeMode::Relaxed`][__link22]
through [`Field::view_with`][__link23] or [`Field::owned_with`][__link24]:

```rust
use http_headers::headers::AcceptEncoding;
use http_headers::{DecodeMode, Field};

let mut headers = http::HeaderMap::new();
headers.insert(
    http::header::ACCEPT_ENCODING,
    http::HeaderValue::from_static("gzip; q = .5"),
);

assert!(AcceptEncoding::view(&headers).is_err());
assert!(AcceptEncoding::view_with(&headers, DecodeMode::Relaxed)?.is_some());
```

Relaxed mode is not a general validation bypass. Each header documents the
additional forms it accepts, and the original field bytes are preserved.

## Sensitive values

Authorization, `Location`, and cookie values are marked sensitive so their
`Debug` representations and compatible sinks do not reveal their contents.
Basic authentication can be read through a borrowed view while reusing
caller-owned decode storage:

```rust
use http::HeaderMap;
use http_headers::Field;
use http_headers::headers::{Authorization, Basic, BasicCredentials};

let mut headers = HeaderMap::new();
headers.insert(
    http::header::AUTHORIZATION,
    http::HeaderValue::from_static("Basic QWxhZGRpbjpvcGVuIHNlc2FtZQ=="),
);

let authorization = Authorization::<Basic>::view(&headers)?.expect("Authorization is present");
let mut credentials = BasicCredentials::new();
let decoded = authorization.extract(&mut credentials)?;
assert_eq!(decoded.username(), b"Aladdin");
credentials.clear();
```

`BasicCredentials` zeroizes decoded bytes when cleared, reused, or dropped.

## Defining a custom single-value header

Implement [`SingleValueField`][__link25] when a custom header is represented by exactly
one field line. The crate then supplies its [`Field`][__link26] implementation,
including borrowed and owned reads, singleton cardinality checks, insertion,
and removal.

```rust
use std::sync::LazyLock;

use http_headers::{
    DecodeError, DecodeErrorKind, FieldName, FieldValue, FieldValueRef, SingleValueField,
};

static REQUEST_ID: LazyLock<FieldName> =
    LazyLock::new(|| FieldName::from_static("x-request-id"));

struct RequestId;

#[derive(Clone, Debug, Eq, PartialEq)]
struct RequestIdOwned(FieldValue);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RequestIdView<'a>(FieldValueRef<'a>);

fn is_token(bytes: &[u8]) -> bool {
    !bytes.is_empty()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(byte))
}

impl SingleValueField for RequestId {
    type View<'a> = RequestIdView<'a>;
    type Owned = RequestIdOwned;

    fn name() -> &'static FieldName {
        &REQUEST_ID
    }

    fn decode_view(value: FieldValueRef<'_>) -> Result<Self::View<'_>, DecodeError> {
        if is_token(value.as_bytes()) {
            Ok(RequestIdView(value))
        } else {
            Err(DecodeError::new(&REQUEST_ID, DecodeErrorKind::InvalidToken))
        }
    }

    fn decode_owned(value: FieldValue) -> Result<Self::Owned, DecodeError> {
        if is_token(value.as_bytes()) {
            Ok(RequestIdOwned(value))
        } else {
            Err(DecodeError::new(&REQUEST_ID, DecodeErrorKind::InvalidToken))
        }
    }

    fn as_field_value(value: &Self::Owned) -> &FieldValue {
        &value.0
    }

    fn into_field_value(value: Self::Owned) -> FieldValue {
        value.0
    }
}
```

## Performance

[`docs/PERF.md`][__link27]
records comparative measurements against `headers 0.4.1`. Borrowed reads
generally avoid allocations; some operations still cost more because this
crate validates more grammar or returns richer semantic types.

## Cargo features

* `headers-all` (enabled by default): all built-in typed header families.
* `headers-authorization`, `headers-cache-control`, `headers-conditional`,
  `headers-content-length`, `headers-content-type`, `headers-cors`,
  `headers-etag`, `headers-location`, `headers-negotiation`, `headers-range`,
  `headers-security`, `headers-set-cookie`, `headers-user-agent`, and
  `headers-websocket`: individual built-in header families.
* `http`: optional adapter for `http::HeaderMap` and the `http` crate’s name,
  value, and method types.
* `serde`: serialization and deserialization for owned headers,
  [`FieldName`][__link28], [`FieldValue`][__link29], and [`sink::EncodedValues`][__link30].

Disable default features to use only the core source, sink, name, and value
APIs, then enable only the header families an application needs.

## What about trailers?

Although this crate is named `http_headers`, it fully supports trailers as well.
The crate doesn’t currently expose any trailer-specific structs however, so you
would need to define those structs and implement the parsers yourself as implementations
of the traits in this crate.

## Alternate crates

This crate is an alternative to the popular [`headers`][__link31] crate.
`http_headers` has the following benefits:

* Faster header parsing and production
* Supports more headers
* Performs more robust validation to avoid downstream surprises
* Supports explicit relaxed parsing options to support common malformed headers
* Supports serde


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/http_headers">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjNhdIQborR2_k_xJd4bTcf2krrNPIcbP72Pw1UdRjkbim_eMDe2BBthYvRhcoQbZ75bZm4izT4buOxogjSu1IIbtSUoTMts1fgbqJKLePQlG_JhZIGCbGh0dHBfaGVhZGVyc2UwLjEuMA
 [__link0]: https://docs.rs/http/latest/http/header/struct.HeaderMap.html
 [__link1]: https://docs.rs/http/latest/http/request/struct.Request.html
 [__link10]: https://docs.rs/http_headers/0.1.0/http_headers/?search=sink::FieldSink
 [__link11]: https://docs.rs/http/latest/http/header/struct.HeaderMap.html
 [__link12]: https://docs.rs/http/latest/http/request/struct.Request.html
 [__link13]: https://docs.rs/http/latest/http/response/struct.Response.html
 [__link14]: https://docs.rs/http_headers/0.1.0/http_headers/?search=sink::FieldSinkExt
 [__link15]: https://docs.rs/http_headers/0.1.0/http_headers/?search=Field::insert
 [__link16]: https://docs.rs/http_headers/0.1.0/http_headers/?search=Field::remove
 [__link17]: https://docs.rs/http_headers/0.1.0/http_headers/?search=FieldName
 [__link18]: https://docs.rs/http_headers/0.1.0/http_headers/?search=FieldValue
 [__link19]: https://docs.rs/http_headers/0.1.0/http_headers/?search=sink::EncodedValues
 [__link2]: https://docs.rs/http/latest/http/response/struct.Response.html
 [__link20]: https://docs.rs/http_headers/0.1.0/http_headers/?search=Field::view
 [__link21]: https://docs.rs/http_headers/0.1.0/http_headers/?search=Field::owned
 [__link22]: https://docs.rs/http_headers/0.1.0/http_headers/?search=DecodeMode::Relaxed
 [__link23]: https://docs.rs/http_headers/0.1.0/http_headers/?search=Field::view_with
 [__link24]: https://docs.rs/http_headers/0.1.0/http_headers/?search=Field::owned_with
 [__link25]: https://docs.rs/http_headers/0.1.0/http_headers/?search=SingleValueField
 [__link26]: https://docs.rs/http_headers/0.1.0/http_headers/?search=Field
 [__link27]: https://github.com/microsoft/oxidizer/blob/main/crates/http_headers/docs/PERF.md
 [__link28]: https://docs.rs/http_headers/0.1.0/http_headers/?search=FieldName
 [__link29]: https://docs.rs/http_headers/0.1.0/http_headers/?search=FieldValue
 [__link3]: https://crates.io/crates/http
 [__link30]: https://docs.rs/http_headers/0.1.0/http_headers/?search=sink::EncodedValues
 [__link31]: https://crates.io/crates/headers
 [__link4]: https://docs.rs/http_headers/0.1.0/http_headers/?search=source::FieldSource
 [__link5]: https://docs.rs/http/latest/http/header/struct.HeaderMap.html
 [__link6]: https://docs.rs/http/latest/http/request/struct.Request.html
 [__link7]: https://docs.rs/http/latest/http/response/struct.Response.html
 [__link8]: https://docs.rs/http_headers/0.1.0/http_headers/?search=Field::view
 [__link9]: https://docs.rs/http_headers/0.1.0/http_headers/?search=Field::owned
