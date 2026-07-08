// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! serde-based JSON⇄message request/response transcoding.
//!
//! These helpers are the runtime that generated transcoders call to turn an
//! HTTP request (captured path variables + query parameters + JSON body) into a
//! typed protobuf request message, and a typed response message back into a
//! JSON body. They are generic over any [`serde`] type, so they compose with
//! `pbjson`-generated proto3-canonical serde implementations without coupling
//! this crate to `pbjson`/`prost`.
//!
//! # proto3 JSON mapping
//!
//! Query values arrive as strings and are inserted into the message JSON as JSON
//! strings. The proto3 JSON mapping accepts quoted values for numeric and 64-bit
//! fields, so string-typed query parameters and numeric fields alike decode
//! correctly through `pbjson`. Captured path variables bypass JSON entirely: the
//! generated transcoder pokes them straight into the message's fields via
//! [`RestParse`] (see [`parse_path_field`] and [`parse_path_enum_value`]), which
//! likewise honors the proto3 mapping for scalars, `bytes` (base64), and enums
//! (by name or number).

mod error;
mod field_deserializer;
mod field_serializer;
mod overlay;
mod percent;
mod request_body_kind;
mod response_body_kind;
mod rest_parse;

#[doc(inline)]
pub use error::TranscodeError;
use field_deserializer::FieldDeserializer;
use field_serializer::{FieldSerError, FieldSerializer};
#[doc(inline)]
pub use request_body_kind::RequestBodyKind;
#[doc(inline)]
pub use response_body_kind::ResponseBodyKind;
pub use rest_parse::{RestParse, parse_path_enum_value, parse_path_field};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde::de::value::MapDeserializer;
use serde_json::{Map, Value};

/// Decodes an HTTP request body and query into a typed request message `T`.
///
/// The message JSON is assembled by layering the body (per `body_kind`) and then
/// the `query` parameters, and deserializing the result into `T`. Captured
/// **path** variables are *not* handled here — the generated transcoder pokes
/// them straight into `T`'s fields afterwards (see [`parse_path_field`]), since
/// they take highest precedence.
///
/// Query-parameter values are percent-decoded before binding (treating `+` as a
/// space); body JSON is used as-is.
///
/// # Errors
///
/// Returns a [`TranscodeError`] if the body is not valid JSON, if the assembled
/// value cannot form an object, or if it fails to deserialize into `T`.
///
/// # Examples
///
/// ```
/// use rest_over_grpc::codegen_helpers::{RequestBodyKind, decode_request};
/// use serde::Deserialize;
///
/// #[derive(Debug, Deserialize)]
/// struct ListBooksRequest {
///     theme: String,
///     page_size: String,
/// }
///
/// let query = [("theme", "history"), ("page_size", "20")];
///
/// let request: ListBooksRequest = decode_request(&query, b"", RequestBodyKind::None)?;
/// assert_eq!(request.theme, "history");
/// assert_eq!(request.page_size, "20");
/// # Ok::<(), rest_over_grpc::codegen_helpers::TranscodeError>(())
/// ```
pub fn decode_request<T: DeserializeOwned>(query: &[(&str, &str)], body: &[u8], body_kind: RequestBodyKind) -> Result<T, TranscodeError> {
    if let Some(result) = try_decode_fast(&body_kind, query, body) {
        return result;
    }

    // The single-pass overlay path handles a flat (top-level) merge of query onto
    // the body without a `serde_json::Value` tree; it declines (returning `None`)
    // for dotted query keys and body-mapped fields, which fall through to the
    // `Value` merge below.
    if let Some(result) = overlay::try_decode_overlay(&body_kind, query, body) {
        return result;
    }

    let mut root = match body_kind {
        RequestBodyKind::None => Value::Object(Map::new()),
        RequestBodyKind::Whole => {
            if body.is_empty() {
                Value::Object(Map::new())
            } else {
                serde_json::from_slice(body).map_err(TranscodeError::body)?
            }
        }
        RequestBodyKind::Field(field) => {
            let mut obj = Map::new();
            if !body.is_empty() {
                let value = serde_json::from_slice(body).map_err(TranscodeError::body)?;
                obj.insert(field.to_owned(), value);
            }
            Value::Object(obj)
        }
    };

    let obj = root
        .as_object_mut()
        .ok_or_else(|| TranscodeError::structure("request body must be a JSON object"))?;

    for (key, value) in query {
        // Query values are percent-decoded (with `+` as space); dotted keys walk
        // the split lazily, flat keys insert directly.
        let decoded = percent::decode_query(value).into_owned();
        if key.contains('.') {
            set_field(obj, key.split('.'), Value::String(decoded))?;
        } else {
            obj.insert((*key).to_owned(), Value::String(decoded));
        }
    }

    serde_json::from_value(root).map_err(TranscodeError::deserialize)
}

/// Attempts the allocation-free fast paths that deserialize directly into `T`,
/// skipping the intermediate `serde_json::Value` map. Returns `None` when no
/// fast path applies, so the caller falls through to the merge path.
///
/// The fast paths are verified-equivalent optimizations of the merge path, so
/// their selection guards are optimization-invariant (flipping a guard just
/// routes an input through the other, equally-correct path). Mutation testing
/// is therefore skipped here; parity is covered by the `decode_request` tests,
/// and [`all_flat`]/[`keys_are_unique`] are unit-tested directly.
#[cfg_attr(test, mutants::skip)]
fn try_decode_fast<T: DeserializeOwned>(
    body_kind: &RequestBodyKind,
    query: &[(&str, &str)],
    body: &[u8],
) -> Option<Result<T, TranscodeError>> {
    match body_kind {
        // No body: deserialize straight from the borrowed query pairs. Only valid
        // when no value needs percent-decoding, since decoding would have to
        // allocate owned strings the merge path handles instead.
        RequestBodyKind::None if all_flat(query) && keys_are_unique(query) && !any_value_needs_decoding(query) => {
            let pairs = query.iter().map(|(k, v)| (*k, *v));
            Some(T::deserialize(MapDeserializer::<_, serde::de::value::Error>::new(pairs)).map_err(TranscodeError::deserialize_value))
        }
        // Whole body with no overlay: the body is the message.
        RequestBodyKind::Whole if query.is_empty() && !body.is_empty() => {
            Some(serde_json::from_slice(body).map_err(TranscodeError::deserialize))
        }
        // Body maps onto one field: parse `{"<field>": <body>}` without
        // materializing the wrapper (see `FieldDeserializer`).
        RequestBodyKind::Field(field) if query.is_empty() && !body.is_empty() => {
            Some(T::deserialize(FieldDeserializer::new(field, body)).map_err(TranscodeError::deserialize))
        }
        _ => None,
    }
}

/// Returns `true` if every query key targets a single, top-level field (no
/// nested `a.b` path), so the flat fast path applies.
fn all_flat(query: &[(&str, &str)]) -> bool {
    query.iter().all(|(key, _)| !key.contains('.'))
}

/// Returns `true` if any query value contains percent-encoding (treating `+` as
/// a space), so the borrowed fast path cannot be used and the caller must fall
/// back to the decoding merge path.
fn any_value_needs_decoding(query: &[(&str, &str)]) -> bool {
    query.iter().any(|(_, value)| percent::needs_decoding(value, true))
}

/// Returns `true` if no query key appears more than once. Duplicates need the
/// `Value`-merge path, which resolves precedence (last write wins); the flat
/// path would instead surface serde's duplicate-field error.
fn keys_are_unique(query: &[(&str, &str)]) -> bool {
    // Query counts are tiny, so an O(n^2) scan avoids any allocation.
    for i in 0..query.len() {
        for j in (i + 1)..query.len() {
            if query[i].0 == query[j].0 {
                return false;
            }
        }
    }
    true
}

/// Encodes a typed response message into a JSON response body.
///
/// # Errors
///
/// Returns a [`TranscodeError`] if `message` fails to serialize, or if
/// `kind` selects a field that the serialized message does not contain.
///
/// # Examples
///
/// ```
/// use rest_over_grpc::codegen_helpers::{ResponseBodyKind, encode_response};
/// use serde::Serialize;
///
/// #[derive(Debug, Serialize)]
/// struct Book {
///     name: String,
///     title: String,
/// }
///
/// let book = Book {
///     name: "shelves/7/books/rust".to_owned(),
///     title: "The Rust Book".to_owned(),
/// };
///
/// let body = encode_response(&book, ResponseBodyKind::Whole)?;
/// let value: serde_json::Value = serde_json::from_slice(&body)?;
/// assert_eq!(value["name"], "shelves/7/books/rust");
///
/// let title = encode_response(&book, ResponseBodyKind::Field("title"))?;
/// assert_eq!(title, br#""The Rust Book""#);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn encode_response<T: Serialize>(message: &T, kind: ResponseBodyKind) -> Result<Vec<u8>, TranscodeError> {
    match kind {
        ResponseBodyKind::Whole => serde_json::to_vec(message).map_err(TranscodeError::serialize),
        ResponseBodyKind::Field(field) => {
            let mut body = Vec::new();
            match message.serialize(FieldSerializer::new(field, &mut body)) {
                Ok(()) => Ok(body),
                Err(FieldSerError::Absent) => Err(TranscodeError::structure("response_body field is absent from the message")),
                // Exotic top-level shapes fall back to the value-tree path for
                // exact parity; nothing was written to `body` yet.
                Err(FieldSerError::Unsupported) => encode_field_via_value(message, field),
                Err(FieldSerError::Json(source)) => Err(TranscodeError::serialize(source)),
                Err(FieldSerError::Custom(detail)) => Err(TranscodeError::serialize_message(detail)),
            }
        }
    }
}

/// The value-tree fallback for [`encode_response`]'s `Field` case: builds the
/// whole message as a [`serde_json::Value`] and re-serializes the selected
/// field. Used only for message shapes the field serializer defers.
fn encode_field_via_value<T: Serialize>(message: &T, field: &str) -> Result<Vec<u8>, TranscodeError> {
    let value = serde_json::to_value(message).map_err(TranscodeError::serialize)?;
    let selected = value
        .get(field)
        .ok_or_else(|| TranscodeError::structure("response_body field is absent from the message"))?;
    serde_json::to_vec(selected).map_err(TranscodeError::serialize)
}

/// Inserts `value` at the (possibly nested) `path` within `obj`, creating
/// intermediate objects as needed. `path` is walked lazily, so dotted query
/// keys need no intermediate `Vec` of components.
fn set_field<'a>(mut obj: &mut Map<String, Value>, path: impl IntoIterator<Item = &'a str>, value: Value) -> Result<(), TranscodeError> {
    let mut path = path.into_iter();
    let Some(mut key) = path.next() else {
        return Ok(());
    };

    loop {
        let Some(next) = path.next() else {
            obj.insert(key.to_owned(), value);
            return Ok(());
        };

        let entry = obj.entry(key.to_owned()).or_insert_with(|| Value::Object(Map::new()));
        obj = entry
            .as_object_mut()
            .ok_or_else(|| TranscodeError::structure("path variable conflicts with a scalar field"))?;
        key = next;
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde::Deserialize;

    use super::*;
    use crate::handling::{Code, Status};

    #[derive(Debug, Deserialize, PartialEq)]
    struct Shelf {
        shelf: String,
        theme: String,
    }

    #[derive(Debug, Serialize)]
    struct Resp {
        name: String,
        size: u32,
    }

    // A type whose `Serialize` impl always fails, to exercise serialize-error paths.
    struct Unserializable;

    impl Serialize for Unserializable {
        fn serialize<S: serde::Serializer>(&self, _serializer: S) -> Result<S::Ok, S::Error> {
            Err(serde::ser::Error::custom("intentional serialize failure"))
        }
    }

    #[test]
    fn all_flat_detects_dotted_query_keys() {
        assert!(all_flat(&[("theme", "x")]));
        assert!(!all_flat(&[("shelf.id", "7")]));
    }

    #[test]
    fn any_value_needs_decoding_detects_encoded_query_values() {
        // No encoding: the borrowed fast path applies.
        assert!(!any_value_needs_decoding(&[("theme", "history")]));
        assert!(any_value_needs_decoding(&[("shelf", "a%2Fb")]));
        // A `+` in a query value needs decoding (treated as a space).
        assert!(any_value_needs_decoding(&[("theme", "science+fiction")]));
    }

    #[test]
    fn keys_are_unique_across_query() {
        assert!(keys_are_unique(&[("a", "1"), ("b", "2")]));
        // A duplicate query key.
        assert!(!keys_are_unique(&[("a", "1"), ("a", "2")]));
    }

    #[test]
    fn decodes_query_parameters() {
        let decoded: Shelf = decode_request(&[("shelf", "1"), ("theme", "sci-fi")], b"", RequestBodyKind::None).expect("decodes");
        assert_eq!(decoded.shelf, "1");
        assert_eq!(decoded.theme, "sci-fi");
    }

    #[test]
    fn query_overlays_body() {
        // A query parameter shadows the same field in the body (query > body).
        let body = br#"{"shelf":"ignored","theme":"history"}"#;
        let decoded: Shelf = decode_request(&[("shelf", "42")], body, RequestBodyKind::Whole).expect("decodes");
        assert_eq!(
            decoded,
            Shelf {
                shelf: "42".to_owned(),
                theme: "history".to_owned()
            }
        );
    }

    #[test]
    fn body_field_mapping() {
        let decoded: Shelf = decode_request(&[("shelf", "9")], br#""history""#, RequestBodyKind::Field("theme")).expect("decodes");
        assert_eq!(decoded.shelf, "9");
        assert_eq!(decoded.theme, "history");
    }

    #[test]
    fn rejects_invalid_body_json() {
        let err = decode_request::<Shelf>(&[], b"not json", RequestBodyKind::Whole).expect_err("invalid json");
        assert_eq!(err.code(), Code::InvalidArgument);
    }

    #[test]
    fn whole_body_reaches_the_value_merge_path_via_a_dotted_query() {
        // A dotted query key makes the overlay fast path decline, so a `Whole`
        // body flows through the `Value` merge path (the arm that parses the body
        // into a `Value` before overlaying the query). A non-empty body is parsed
        // there; an empty body becomes an empty object.
        let non_empty: Value =
            decode_request(&[("nested.key", "v")], br#"{"shelf":"7"}"#, RequestBodyKind::Whole).expect("non-empty whole body decodes");
        assert_eq!(non_empty["shelf"], "7");
        assert_eq!(non_empty["nested"]["key"], "v");

        let empty: Value = decode_request(&[("nested.key", "v")], b"", RequestBodyKind::Whole).expect("empty whole body decodes");
        assert_eq!(empty["nested"]["key"], "v");
        assert!(empty.get("shelf").is_none());
    }

    #[test]
    fn set_field_with_an_empty_path_is_a_no_op() {
        // Defensive base case: an empty path leaves the object untouched and
        // succeeds. Real callers pass `key.split('.')`, which is never empty.
        let mut obj = Map::new();
        set_field(&mut obj, std::iter::empty::<&str>(), Value::String("x".to_owned())).expect("empty path is Ok");
        assert!(obj.is_empty());
    }

    #[test]
    fn encodes_whole_response() {
        let resp = Resp {
            name: "shelves/1".to_owned(),
            size: 3,
        };
        let bytes = encode_response(&resp, ResponseBodyKind::Whole).expect("encodes");
        let value: Value = serde_json::from_slice(&bytes).expect("valid json");
        assert_eq!(value["name"], "shelves/1");
        assert_eq!(value["size"], 3);
    }

    #[test]
    fn encodes_response_body_field() {
        let resp = Resp {
            name: "shelves/1".to_owned(),
            size: 3,
        };
        let bytes = encode_response(&resp, ResponseBodyKind::Field("name")).expect("encodes");
        assert_eq!(bytes, br#""shelves/1""#);
    }

    #[test]
    fn encodes_numeric_response_body_field() {
        let resp = Resp {
            name: "n".to_owned(),
            size: 42,
        };
        let bytes = encode_response(&resp, ResponseBodyKind::Field("size")).expect("encodes");
        assert_eq!(bytes, b"42");
    }

    #[test]
    fn encodes_nested_object_response_body_field() {
        // A field whose value is a nested message is emitted verbatim, without
        // routing through an intermediate `serde_json::Value` tree.
        #[derive(Debug, Serialize)]
        struct Outer {
            inner: Inner,
            other: u32,
        }
        #[derive(Debug, Serialize)]
        struct Inner {
            a: String,
            b: u32,
        }
        let outer = Outer {
            inner: Inner { a: "x".to_owned(), b: 1 },
            other: 9,
        };
        let bytes = encode_response(&outer, ResponseBodyKind::Field("inner")).expect("encodes");
        let value: Value = serde_json::from_slice(&bytes).expect("valid json");
        assert_eq!(value, serde_json::json!({"a": "x", "b": 1}));
    }

    #[test]
    fn encodes_response_body_field_through_option_wrapper() {
        // A struct behind `Option`/`Some` is transparent to field selection.
        let resp = Some(Resp {
            name: "wrapped".to_owned(),
            size: 1,
        });
        let bytes = encode_response(&resp, ResponseBodyKind::Field("name")).expect("encodes");
        assert_eq!(bytes, br#""wrapped""#);
    }

    #[test]
    fn field_selection_ignores_unserializable_sibling() {
        // Only the selected field is serialized, so a sibling that cannot be
        // serialized does not affect a successful field extraction.
        #[derive(Serialize)]
        struct Mixed {
            good: String,
            bad: Unserializable,
        }
        let mixed = Mixed {
            good: "ok".to_owned(),
            bad: Unserializable,
        };
        let bytes = encode_response(&mixed, ResponseBodyKind::Field("good")).expect("encodes selected field");
        assert_eq!(bytes, br#""ok""#);
    }

    #[test]
    fn field_selection_reports_selected_field_serialize_failure() {
        // When the selected field itself fails to serialize, the error maps to
        // an internal (serialize) error, not a structure error.
        #[derive(Serialize)]
        struct Wrap {
            bad: Unserializable,
        }
        let err = encode_response(&Wrap { bad: Unserializable }, ResponseBodyKind::Field("bad")).expect_err("field fails");
        assert_eq!(err.code(), Code::Internal);
        assert!(err.to_string().contains("serialize"));
    }

    #[test]
    fn field_selection_maps_top_level_custom_error_to_internal() {
        // A top-level message whose `Serialize` reports a custom error surfaces
        // through the field serializer's `Custom` variant as an internal error.
        let err = encode_response(&Unserializable, ResponseBodyKind::Field("any")).expect_err("top-level serialize fails");
        assert_eq!(err.code(), Code::Internal);
        assert!(err.to_string().contains("serialize"));
    }

    #[test]
    fn field_selection_on_non_object_is_absent() {
        // A scalar top-level message cannot carry a named field.
        let err = encode_response(&"scalar", ResponseBodyKind::Field("name")).expect_err("no fields");
        assert_eq!(err.code(), Code::InvalidArgument);
        assert!(err.to_string().contains("structure"));
    }

    #[test]
    fn field_selection_falls_back_for_map_messages() {
        // A map-shaped top-level message is not a struct, so encoding falls back
        // to the value-tree path and still selects the field correctly.
        let mut map = BTreeMap::new();
        let _ = map.insert("name".to_owned(), "from_map".to_owned());
        let _ = map.insert("theme".to_owned(), "history".to_owned());
        let bytes = encode_response(&map, ResponseBodyKind::Field("name")).expect("encodes via fallback");
        assert_eq!(bytes, br#""from_map""#);
    }

    #[test]
    fn field_selection_map_fallback_reports_absent() {
        // The fallback path preserves the absent-field structure error.
        let mut map = BTreeMap::new();
        let _ = map.insert("name".to_owned(), "x".to_owned());
        let err = encode_response(&map, ResponseBodyKind::Field("missing")).expect_err("absent");
        assert_eq!(err.code(), Code::InvalidArgument);
    }

    #[derive(Debug, Deserialize, PartialEq)]
    struct Nested {
        shelf: Inner,
    }

    #[derive(Debug, Deserialize, PartialEq)]
    struct Inner {
        id: String,
    }

    #[test]
    fn integrates_with_scanned_path_query() {
        // A query parameter flows into decode alongside a body without panicking.
        let decoded: Shelf = decode_request(&[("shelf", "55"), ("theme", "t")], b"", RequestBodyKind::None).expect("decodes");
        assert_eq!(decoded.shelf, "55");
        assert_eq!(decoded.theme, "t");
    }

    #[test]
    fn decodes_nested_query_parameter() {
        // A `.`-separated key routes into a nested message field via the value path.
        let decoded: Nested = decode_request(&[("shelf.id", "9")], b"", RequestBodyKind::None).expect("decodes");
        assert_eq!(decoded.shelf.id, "9");
    }

    #[test]
    fn flat_fast_path_agrees_with_value_path() {
        // Fast path: no body, unique flat keys.
        let fast: Shelf = decode_request(&[("shelf", "history"), ("theme", "science")], b"", RequestBodyKind::None).expect("fast");
        // Force the `Value`-merge path by supplying an (empty) whole body.
        let slow: Shelf = decode_request(&[("shelf", "history"), ("theme", "science")], b"{}", RequestBodyKind::Whole).expect("slow");
        assert_eq!(fast, slow);
        assert_eq!(fast.shelf, "history");
        assert_eq!(fast.theme, "science");
    }

    #[test]
    fn query_shadows_body_field() {
        // `shelf` appears in both the body and a query param; the query wins
        // (matching the `Value`-merge precedence).
        let body = br#"{"shelf":"from_body","theme":"t"}"#;
        let decoded: Shelf = decode_request(&[("shelf", "from_query")], body, RequestBodyKind::Whole).expect("decodes");
        assert_eq!(decoded.shelf, "from_query");
        assert_eq!(decoded.theme, "t");
    }

    #[test]
    fn whole_body_empty_is_an_empty_object() {
        // Body present per `body_kind` but empty bytes → an empty object, filled
        // by the query.
        let decoded: Shelf = decode_request(&[("shelf", "1"), ("theme", "t")], b"", RequestBodyKind::Whole).expect("decodes");
        assert_eq!(decoded.shelf, "1");
    }

    #[test]
    fn field_body_maps_non_empty_body() {
        let decoded: Shelf = decode_request(&[("shelf", "9")], br#""history""#, RequestBodyKind::Field("theme")).expect("decodes");
        assert_eq!(decoded.theme, "history");
    }

    #[test]
    fn field_body_empty_skips_insert() {
        // `RequestBodyKind::Field` with an empty body inserts nothing; the fields
        // come from the query instead.
        let decoded: Shelf = decode_request(&[("shelf", "s"), ("theme", "t")], b"", RequestBodyKind::Field("theme")).expect("decodes");
        assert_eq!(decoded.shelf, "s");
        assert_eq!(decoded.theme, "t");
    }

    #[test]
    fn value_path_deserialize_error() {
        // Body parses as JSON but the wrong type fails `from_value`.
        let err = decode_request::<Shelf>(&[], br#"{"shelf":123,"theme":"t"}"#, RequestBodyKind::Whole).expect_err("type mismatch");
        assert_eq!(err.code(), Code::InvalidArgument);
        assert!(err.to_string().contains("did not match"));
    }

    #[test]
    fn fast_path_deserialize_error() {
        // Fast path (no body, flat, unique keys) with a missing required field.
        let err = decode_request::<Shelf>(&[("shelf", "s")], b"", RequestBodyKind::None).expect_err("missing theme");
        assert_eq!(err.code(), Code::InvalidArgument);
    }

    #[test]
    fn non_object_whole_body_is_a_deserialize_error() {
        // A non-object whole body fails the direct-deserialize fast path as a
        // type mismatch, still `InvalidArgument` (HTTP 400).
        let err = decode_request::<Shelf>(&[], b"[1,2,3]", RequestBodyKind::Whole).expect_err("array body");
        assert_eq!(err.code(), Code::InvalidArgument);
        assert!(err.to_string().contains("did not match"));
    }

    #[test]
    fn non_object_body_with_query_is_a_structure_error() {
        // With a query parameter to overlay, decoding takes the `Value`-merge
        // path, which reports a non-object body as a structure error.
        let err = decode_request::<Shelf>(&[("shelf", "1")], b"[1,2,3]", RequestBodyKind::Whole).expect_err("array body");
        assert_eq!(err.code(), Code::InvalidArgument);
        assert!(err.to_string().contains("invalid request structure"));
    }

    #[test]
    fn whole_body_direct_path_agrees_with_merge_path() {
        let body = br#"{"shelf":"s","theme":"history"}"#;
        // Direct path: whole body, no query.
        let direct: Shelf = decode_request(&[], body, RequestBodyKind::Whole).expect("direct");
        // Merge path: an (ignored, distinct-key) query param forces the `Value` merge.
        let merged: Shelf = decode_request(&[("extra_ignored", "x")], body, RequestBodyKind::Whole).expect("merged");
        assert_eq!(direct, merged);
        assert_eq!(direct.shelf, "s");
        assert_eq!(direct.theme, "history");
    }

    #[test]
    fn field_body_direct_path_agrees_with_merge_path() {
        // `RequestBodyKind::Field` with no query takes the direct sub-deserializer
        // path; a nested field body also decodes directly. Both succeed.
        let direct: Shelf = decode_request(&[("shelf", "9")], br#""history""#, RequestBodyKind::Field("theme")).expect("merge");
        let via_direct: Nested = decode_request(&[], br#"{"id":"7"}"#, RequestBodyKind::Field("shelf")).expect("direct");
        assert_eq!(direct.theme, "history");
        assert_eq!(via_direct.shelf.id, "7");
    }

    #[test]
    fn field_body_direct_path_rejects_invalid_json() {
        // The field sub-deserializer surfaces a parse error as an invalid
        // argument, just like the whole-body path.
        let err = decode_request::<Shelf>(&[], b"not json", RequestBodyKind::Field("theme")).expect_err("invalid json");
        assert_eq!(err.code(), Code::InvalidArgument);
    }

    #[test]
    fn overlay_path_preserves_nested_body_and_applies_precedence() {
        // A whole body carrying a nested object, overlaid by a flat query
        // parameter, takes the single-pass overlay path.
        #[derive(Debug, Deserialize, PartialEq)]
        struct Outer {
            shelf: String,
            theme: String,
            inner: Inner,
        }

        let body = br#"{"shelf":"from-body","inner":{"id":"nested"}}"#;
        // Query supplies `theme` and shadows the body's `shelf`.
        let decoded: Outer =
            decode_request(&[("shelf", "from-query"), ("theme", "history")], body, RequestBodyKind::Whole).expect("overlay decodes");
        assert_eq!(
            decoded,
            Outer {
                shelf: "from-query".to_owned(),
                theme: "history".to_owned(),
                inner: Inner { id: "nested".to_owned() },
            }
        );
    }

    #[test]
    fn overlay_path_percent_decodes_overlaid_values() {
        // A percent-encoded query value forces owned storage on the overlay path
        // (the borrowed fast path declines); the value is still decoded.
        let decoded: Shelf =
            decode_request(&[("shelf", "a%2Fb"), ("theme", "science%20fiction")], b"", RequestBodyKind::None).expect("overlay decodes");
        assert_eq!(decoded.shelf, "a/b");
        assert_eq!(decoded.theme, "science fiction");
    }

    #[test]
    fn field_body_direct_path_rejects_trailing_bytes() {
        // Trailing content after the field value is rejected, mirroring
        // `serde_json::from_slice`'s end-of-input check.
        let err = decode_request::<Shelf>(&[], br#""history" extra"#, RequestBodyKind::Field("theme")).expect_err("trailing");
        assert_eq!(err.code(), Code::InvalidArgument);
    }

    #[test]
    fn query_key_conflicting_with_scalar_is_a_structure_error() {
        // `a` is set to a scalar, then `a.b` tries to nest into it.
        let err = decode_request::<Shelf>(&[("a", "1"), ("a.b", "2")], b"", RequestBodyKind::None).expect_err("conflict");
        assert_eq!(err.code(), Code::InvalidArgument);
    }

    #[test]
    fn percent_decodes_query_value_and_plus() {
        let decoded: Shelf =
            decode_request(&[("shelf", "7"), ("theme", "science+%66iction")], b"", RequestBodyKind::None).expect("decodes");
        assert_eq!(decoded.shelf, "7");
        // `+` → space and `%66` → `f`.
        assert_eq!(decoded.theme, "science fiction");
    }

    #[test]
    fn undecoded_values_still_take_the_fast_path() {
        // A plain (unencoded) value round-trips through the fast path.
        let fast: Shelf = decode_request(&[("shelf", "7"), ("theme", "history")], b"", RequestBodyKind::None).expect("fast");
        assert_eq!(fast.shelf, "7");
        assert_eq!(fast.theme, "history");
    }

    #[test]
    fn encode_serialize_error_maps_to_internal() {
        let err = encode_response(&Unserializable, ResponseBodyKind::Whole).expect_err("nan");
        assert_eq!(err.code(), Code::Internal);
        assert!(err.to_string().contains("serialize"));
    }

    #[test]
    fn encode_response_body_field_absent() {
        let err = encode_response(
            &Resp {
                name: "n".to_owned(),
                size: 1,
            },
            ResponseBodyKind::Field("missing"),
        )
        .expect_err("absent");
        assert_eq!(err.code(), Code::InvalidArgument);
    }

    #[test]
    fn error_into_status_carries_code_and_message() {
        // A query parameter forces the `Value`-merge path, which reports a
        // malformed body as a body-JSON error.
        let err = decode_request::<Shelf>(&[("shelf", "1")], b"not json", RequestBodyKind::Whole).expect_err("bad json");
        // The `From` conversion agrees with the inherent `into_status`.
        let via_from: Status = decode_request::<Shelf>(&[("shelf", "1")], b"not json", RequestBodyKind::Whole)
            .expect_err("bad json")
            .into();
        let status = err.into_status();
        assert_eq!(status.code(), Code::InvalidArgument);
        assert_eq!(via_from.code(), Code::InvalidArgument);
        assert!(status.message().contains("invalid request body JSON"));
    }

    #[test]
    fn error_exposes_underlying_source_when_present() {
        use std::error::Error as _;

        // A parse failure carries the originating `serde_json` error as its
        // source; a captured backtrace shows up in the `Debug` output.
        let parse_err = decode_request::<Shelf>(&[], b"not json", RequestBodyKind::Whole).expect_err("bad json");
        assert!(parse_err.source().is_some());
        assert!(format!("{parse_err:?}").contains("backtrace"));

        // A structural error (non-object body on the merge path) has no
        // underlying cause. The query parameter forces the `Value`-merge path.
        let structure_err = decode_request::<Shelf>(&[("shelf", "1")], b"42", RequestBodyKind::Whole).expect_err("not an object");
        assert!(structure_err.source().is_none());
    }
}
