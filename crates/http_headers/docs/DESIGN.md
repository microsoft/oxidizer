# Design

`http_headers` is a typed header layer with its own core abstractions. Its
central design choice is to preserve HTTP wire structure while letting callers
choose between borrowed parsing, owned values, and reusable Basic credential
storage.

## Core abstractions and container adapters

The core owns these abstractions and depends on no external header crate:

- `FieldValue` stores validated wire bytes inline through 64 bytes and uses
  shared `bytes::Bytes` storage for longer values. `FieldValueRef<'a>` is the
  borrowed counterpart every decoder observes.
- `FieldName` is the single map-key, wire-name, and typed-field name type. It
  has one variant for every recognized standard header and `Custom(Arc<str>)`
  for other names. Runtime parsing normalizes custom names to lowercase.
  Typed APIs use `&'static FieldName`, so known variants and cached custom
  names are passed without cloning their owned representation.
- `source::FieldSource` supplies the `source::FieldLines` stored for a
  typed field, and `sink::FieldSink` stores them. Each generic operation
  obtains its key from `Field::name`: a container that indexes well-known
  headers reads `FieldName::index` and answers without hashing or comparing
  anything, while any other container compares the name's bytes.
- `FieldSource` carries the typed read API, while `FieldSink` carries the
  typed write API. Typed methods look a field up by `Field::name` alone, and
  because that name is a compile-time constant the indexed path folds away
  during monomorphization. Nothing on a decoding path is a trait object.

Container integrations sit outside these abstractions. The optional `http`
feature implements both traits for `http::HeaderMap` and supplies conversions
at that boundary. Values retained from a `HeaderMap` may use an internal
adapter-specific representation to share its storage; this does not alter the
public `FieldValue` model or typed-header behavior. Other integrations can
implement the same source and sink contracts without depending on `http`;
disabling the feature removes that dependency without removing any typed
header.

## Owned headers and borrowed views

Every `Field` definition associates a generic `View<'a>` and an `Owned`
representation. `view` returns the view, which can borrow
`FieldValueRef` storage directly from the header source.
`owned` invokes the implementation's owned path directly.

This split avoids forcing a clone for read-only access while still giving
callers a type that can outlive the map. Some headers, such as
`ContentLength`, are small values and use the same copyable type for both
forms.

`Field::insert` lets each implementation move its validated storage into the
destination without exposing an intermediate encoding operation.

## FieldLines and encoded values

`FieldLines<'a>` borrows the field lines a field source already stores — a
slice of `FieldValue`s, borrowed `FieldValueRef`s into arbitrary transport
storage, one parsed line, or the field lines of an adapted
`http::HeaderMap` — rather than collecting or joining them. The set of
representations is closed, so iteration dispatches statically. It therefore
retains each field-line boundary and insertion order. Parsers can require
exactly one value, reiterate repeated values, or scan comma- and
semicolon-delimited items across all lines. Delimited scanning trims optional
whitespace and respects quoted delimiters within each physical line. An item
cannot span lines because iterator items borrow one contiguous source line.

`FieldSource::lines` returns `None` for absence and
`Some(FieldLines)` for presence. `FieldLines` therefore always contains
at least one raw field line; a zero-length line remains present and is decoded
according to the typed header's grammar rather than being confused with
absence. Slice, borrowed, and `http::HeaderMap` constructors return `None` when
given no lines.

Custom sources are bounded at the shared decode boundary: one field name may
contribute at most 64 KiB across at most 128 field lines, and one delimited
decode may yield at most 1,024 list items. Owned decoding checks the byte and
line budgets before copying source data, while `DelimitedItems` applies the
item budget before yielding another item. Specialized list parsers invoke the
same `FieldLines` item-budget check before their optimized scans. Opaque
single-value parsers do not interpret delimiters as list members. The
`http::HeaderMap` representation is exempt because that adapter exposes
already validated values and is not a custom `FieldSource`.

This distinction is required for `Set-Cookie`, where each cookie remains a
separate field line. List-valued headers can still treat multiple lines as one
logical list without first allocating a flattened string.

`EncodedValues` mirrors that model on output. A singleton is stored directly
in an `Option<FieldValue>` with no `Vec` allocation. Repeated values can be
appended, while `from_vec` adopts an owned vector without reallocating it.

## Fallible, atomic map operations

`FieldSource` provides raw container lookup primitives. `FieldSink` adds
`set_values`, `append_values`, and `remove_values` as the raw mutation
primitives. Requiring append separately lets the provided `append_encoded`
operation encode its complete delta before one atomic, delta-proportional
mutation. `Field` provides strict `view` and `owned` conveniences;
implementations supply `view_with`, `owned_with`, and `insert`. Its provided
`remove` operation deletes matching field lines without parsing them.

`Field::insert` returns `Result<(), InsertError>`. It encodes before mutation
and reserves or obtains the destination entry before replacing an existing
value. If the container has reached its maximum capacity, insertion reports the
error and leaves the existing field lines unchanged. An empty encoding
removes the existing header.

`Field::remove` performs no decoding and therefore cannot fail because a
stored value is malformed.

## Errors and secret-safe diagnostics

`DecodeError` records a copyable static reference to `FieldName`, a non-exhaustive
`DecodeErrorKind`, and an optional zero-based field-value index. It
deliberately does not retain or print raw field bytes because headers may
contain credentials, cookies, signed URLs, or other secrets.

Sensitive built-in types set `FieldValue::set_sensitive` where appropriate.
Their `Debug` implementations, along with those for `FieldLines`,
`BasicCredentials`, and encoded collections, report structure or counts rather
than contents. Error handling remains structured without making secret data
part of routine logs.

## Reusable Basic credentials

`BasicCredentials` owns decoded Basic authorization bytes and reuses its
allocation across requests. `AuthorizationOwned<Basic>::extract` and
`AuthorizationView<'_, Basic>::extract` return a reference to that storage,
preventing reuse while the username and password are borrowed. Previous
credentials are zeroized before extraction, when cleared, and on drop.
`BasicCredentials::clear` retains capacity up to a caller-configurable limit
and drops an oversized allocation. The default retention cap is 64 KiB.

## Downstream extension contract

Downstream crates can implement `Field` directly when they need repeated
values, custom ownership, caching, or a nonstandard encoding. Implementations
must:

1. return a `&'static FieldName`; custom headers can initialize one
   `FieldName::Custom` lazily with `std::sync::LazyLock`;
2. validate grammar and cardinality when decoding;
3. return views that borrow only for their declared lifetime;
4. produce only valid `FieldValue` objects when encoding; and
5. preserve any security invariant, including sensitivity markings and
   secret-safe `Debug`.

`SingleValueField` is the preferred adapter for one-field-value types. It
supplies exact singleton cardinality and move-based encoding; the downstream
`view` function validates inbound values, while constructors must establish
the same invariant for owned values.

Built-in headers validate through a private `validate` module that wraps safe,
shared primitives for token and field-value validation, token-byte checks,
ASCII case-insensitive comparison, and scanning for list/quote/whitespace
bytes. It is an internal detail built on the rustdoc-hidden
`http_headers_simd` companion crate. That crate is published only as an
implementation dependency and is not a supported downstream API. Custom
headers implement their own validation instead of depending on it.

## Protocol behavior

The parsers preserve byte-level RFC behavior instead of treating every field
as a Unicode string.

- **Cache-Control:** recipients ignore empty comma-list members, including an
  entirely empty logical list. Sender constructors and the builder require at
  least one nonempty valid directive. Directive values are exposed as bytes;
  `value_str` is an explicit fallible UTF-8 conversion.
- **Quoted strings and `obs-text`:** field-value validation accepts bytes
  `0x80..=0xff`, and Cache-Control and Content-Type quoted strings retain them.
  APIs return raw bytes where UTF-8 is not guaranteed.
- **Content-Type:** empty parameter slots such as trailing `;` or repeated
  semicolons are tolerated. Parameter names and values must have no optional
  whitespace around `=`.
- **Location:** values are RFC 3986 URI-references, including absolute,
  relative, fragment-only, and empty references. Validation is delegated to
  `fluent-uri` with IPvFuture support rather than to an HTTP URI model with
  different restrictions.
- **Content-Length:** repeated field lines and comma-separated duplicates are
  accepted only when every decimal value agrees. Conflicts, invalid digits,
  and `u64` overflow are errors.
- **Set-Cookie:** repeated field lines are retained and encoded independently;
  they are never parsed as a comma list.
- **Negotiation:** media ranges, codings, languages, methods, and field names
  remain allocation-free list iterators. `Host` separates host and optional
  port without normalizing the authority.
- **Conditional requests and dates:** entity-tag lists preserve wildcard and
  weak/strong semantics. HTTP dates accept the three recipient date formats;
  semantic constructors emit IMF-fixdate. `If-Range` accepts only a strong
  entity tag or an HTTP date.
- **Ranges:** byte-range and content-range components are parsed without
  flattening field lines. Unknown syntactically valid range units remain
  visible rather than being rewritten as bytes.
- **CORS:** origins, methods, and field-name lists are syntax types, not
  authorization policy. In particular, wildcard and credential combinations
  remain the application's policy decision.
- **WebSocket:** key and accept values require canonical base64 lengths;
  `SecWebSocketAcceptOwned::from_key` computes the RFC handshake digest. Extension
  parameters preserve quoted wire data and reject whitespace around `=`.
- **Security fields:** HSTS is structured and requires exactly one `max-age`;
  CSP deliberately remains an opaque safe field value; Referrer-Policy keeps
  unknown future tokens while selecting the last recognized fallback.

## Inline storage

`FieldValue` stores up to 64 bytes inline. Longer values use shared
`bytes::Bytes` storage, and values retained from the optional `http` adapter
can retain the source `HeaderValue` without copying its private buffer.
Sensitivity is stored beside every representation and does not affect
equality, ordering, or hashing.

`SmallVec` removes common temporary allocations in the generic encoding sink,
the `http::HeaderMap` adapter, and Cache-Control storage. `CompactString`
stores extension directives in the Cache-Control and HSTS builders. These
choices are covered by the `storage` metabench target and by unit tests at
their inline/spill boundaries; they are implementation details rather than
layout guarantees.

## SIMD dispatch and unsafe isolation

The token, token68, field-value, and interesting-byte scanners use scalar Rust
below 16 bytes on x86 and x86-64, and below 32 bytes on other architectures.
Token-list, Base64, and URI scanners use a 16-byte threshold on every
architecture; ASCII case-insensitive equality uses 32 bytes. At or above each
scanner's threshold, dispatch selects:

- SSE2 on x86-64, where it is part of the architecture baseline, with an
  SSE4.2 `pcmpestrm` fast path for token68 when runtime detection succeeds;
- SSE2 on x86 when runtime detection (or the compile-time target feature
  without `std`) confirms support, again preferring SSE4.2 for token68;
- NEON on AArch64 when runtime detection (or the compile-time target feature
  without `std`) confirms support; or
- the scalar implementation on all other targets and whenever the optimized
  backend is unavailable.

Vector loops process 16-byte lanes and pass tails to the scalar reference
implementation. Differential properties check backend equivalence, including
lengths around vector and dispatch boundaries.

The user-facing `http_headers` crate declares `#![forbid(unsafe_code)]`.
Project-owned pointer loads, intrinsics, and `target_feature` functions live
only in the rustdoc-hidden `http_headers_simd` crate, which denies unsafe
operations inside unsafe functions unless they are explicit. Its outward API
is safe, so target-feature preconditions and pointer bounds cannot leak into
`http_headers`.

## Deliberate API decisions

The following surfaces are deliberate rather than oversights.

**`SetCookieOwned` stays construction-fallible; no `FromIterator`/`Extend`.**
`SetCookieOwned::push` and `push_str` validate every field value
against `SET_COOKIE`'s nonempty-field-value grammar and reject one that
fails, and that stays the only way to build one up. `EncodedValues`'
`FromIterator`/`Extend` adopt trusted, already-encoded `FieldValue` storage,
not attacker-reachable cookie content; a `FromIterator`/
`Extend` impl for `SetCookieOwned` would need to either silently drop invalid
values (breaking the invariant every stored value passes validation) or
panic (turning untrusted request- or response-adjacent data into a crash
surface), and neither is acceptable for a type whose whole job is holding
values that survive round-tripping intact. `iter()`, `IntoIterator for
SetCookieOwned`, `&SetCookieOwned`, and `&mut SetCookieOwned` are safe because reading and
encoding existing values cannot invalidate them. Construction goes through
`push`/`push_str` and their `Result`, not an infallible collection trait.

**Built-in descriptors forward essential `Field` operations.** Every built-in
descriptor exposes inherent `view`, `owned`, `insert`, and `remove` methods so
callers can discover and invoke its essential operations without importing the
`Field` trait. These methods are generated from the same built-in field list
and delegate directly to `Field`, keeping their behavior and signatures tied
to the trait implementation instead of maintaining hand-written duplicates.
`Field` remains the generic contract and the extension point for downstream
field descriptors.

**Type names follow RFC 9110; the crate name follows common usage.** RFC 9110
calls the genus a *field*: a field has a field name and a field value, and a
field that appears in the header section is a *header field* (Section 6.3),
which the specification itself notes is called a "header" only colloquially.
The types therefore say `FieldName`, `FieldValue`, `FieldSource`, and
`FieldSink`, because none of them is specific to the header section — the same
`FieldSource` is correct over a trailer section. The crate stays
`http_headers` because a crate name is a domain label in the reader's
vocabulary rather than a node in the specification's taxonomy, and because
`FieldName` no longer collides with `http::HeaderName` at any import site.
Headers of the header section keep the word "header" in prose and in the
`headers` module, where it is accurate.

**Types are grouped by role, and each has exactly one path.** Concrete
headers, their views, builders, scheme markers, and related enums live under
`http_headers::headers`. The read-path extension points a custom container
implements to supply field values live under `http_headers::source`, and the
write-path ones it implements to store them live under `http_headers::sink`.
The crate root holds only the vocabulary every user names: field values,
field names, decode errors and modes, and the `Field` traits. No type is
re-exported from two paths, so there is one way to import each name and
`cargo doc` lists it once.

**One unconditional header-family set, no per-family dependency features.**
`sha1`, `fluent-uri`, `httpdate`, and `base64` stay unconditional
dependencies rather than gaining one Cargo feature per header family. The
`http` and `serde` features provide integrations rather than selecting header
families. Gating these four dependencies would multiply the
`cargo hack --each-feature` and docs.rs feature-power-set combinations, and
feature unification means one dependent enabling a family
silently compiles it for every other crate in the same build — a surprise
users would need this document to explain either way. A consumer that reads
only `Content-Type` and `Content-Length` pays a modest, fixed compile-time
and binary-size cost for four small, non-`unsafe`, pure-Rust dependencies
with no transitive weight comparable to `http_headers_simd`'s own footprint.
The Cargo default feature set is empty, but all built-in header families are
unconditional; per-family features are not planned.

**Opaque owned headers retain `FieldValue`.** Their owned forms clone and hold
the validated `FieldValue` rather than introducing a second opaque-byte
representation. This preserves sensitivity metadata, inline/shared storage,
and round-trip behavior. Callers that only inspect a value should use `view`
to borrow source storage instead.

**Accessors keep re-validating UTF-8; views keep returning `Result`.**
`LocationView::as_str` and the equivalent per-item CORS,
`AcceptRanges`, and security-header accessors keep re-running
`str::from_utf8` over bytes the decoder already proved to be ASCII, rather
than carrying a pre-validated `&str` in the view and dropping the `Result`
those accessors return. Storing `&str` instead of `&[u8]` in a view is an
accessor-signature change for every caller matching on today's
`Result<&str, Utf8Error>` return; per this item's own escape clause, the
`from_utf8` pass is a validation over an already-short, already-ASCII slice
next to work — grammar validation, quote/whitespace scanning — that has
already walked the same bytes once, so it is not the dominant cache-and-clone
cost. Kept as-is; accessor signatures and their `Result` returns are
preserved.
