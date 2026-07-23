# TODO

## Query codec follow-ups

The direct `FromQuery` and `ToQuery` codecs are implemented. Potential extensions
are an optional Serde migration adapter, custom per-field codecs, type and const
generic schemas, configurable scalar-duplicate and key-only policies, and
compile-time collision diagnostics across flattened schemas.

## Path canonicalization

Add an explicit path-preparation API that runs before route matching. A
`PreparedPath` should borrow unchanged input and own rewritten input, expose
whether canonicalization changed the path so callers can redirect, and keep the
prepared value alive while resolved routes borrow from it.

Provide policies for preserving, rejecting, or normalizing repeated slashes,
dot segments, and trailing slashes, plus preserving or rejecting encoded
separators. Query or fragment delimiters and malformed percent escapes should be
rejected. Percent-encoded `/`, `\`, `.`, and `..` must never become routing
structure through decoding. Include exact and strict presets, with core route
matching remaining exact and normalization explicitly selected by the caller.

An initial API shape could be:

```rust
pub mod path {
    use alloc::borrow::Cow;

    pub struct PreparedPath<'p> {
        value: Cow<'p, str>,
        changed: bool,
    }

    impl<'p> PreparedPath<'p> {
        pub fn new(path: &'p str, policy: PathPolicy) -> Result<Self, Error>;
        pub fn as_str(&self) -> &str;
        pub const fn was_changed(&self) -> bool;
    }

    pub struct PathPolicy {
        pub repeated_slashes: RepeatedSlashes,
        pub dot_segments: DotSegments,
        pub trailing_slash: TrailingSlash,
        pub encoded_separators: EncodedSeparators,
    }

    impl PathPolicy {
        pub const EXACT: Self;
        pub const STRICT: Self;
    }

    pub enum RepeatedSlashes {
        Preserve,
        Reject,
        Collapse,
    }

    pub enum DotSegments {
        Preserve,
        Reject,
        Remove,
    }

    pub enum TrailingSlash {
        Preserve,
        Reject,
        Remove,
    }

    pub enum EncodedSeparators {
        Preserve,
        Reject,
    }
}
```

The caller would retain ownership of the prepared path while using any route
that borrows from it:

```rust
let prepared = routerama::path::PreparedPath::new(
    uri_path,
    routerama::path::PathPolicy::STRICT,
)?;

if prepared.was_changed() {
    return redirect_permanently(prepared.as_str());
}

let route = resolver.resolve(method, prepared.as_str())?;
```
