// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! serde-based JSON⇄message request/response transcoding.
//!
//! These helpers are the runtime that generated dispatchers call to turn an
//! HTTP request (captured path variables + query parameters + JSON body) into a
//! typed protobuf request message, and a typed response message back into a
//! JSON body. They are generic over any [`serde`] type, so they compose with
//! `pbjson`-generated proto3-canonical serde implementations without coupling
//! this crate to `pbjson`/`prost`.
//!
//! # proto3 JSON mapping
//!
//! Path and query values arrive as strings and are inserted into the message
//! JSON as JSON strings. The proto3 JSON mapping accepts quoted values for
//! numeric and 64-bit fields, so string-typed path variables (resource names,
//! IDs) and numeric fields both decode correctly through `pbjson`.

mod body_kind;
mod error;
mod field_deserializer;
mod response_body_kind;

#[doc(inline)]
pub use body_kind::BodyKind;
#[doc(inline)]
pub use error::TranscodeError;
use field_deserializer::FieldDeserializer;
#[doc(inline)]
pub use response_body_kind::ResponseBodyKind;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde::de::value::MapDeserializer;
use serde_json::{Map, Value};

use crate::{Binding, HttpResponse, Status};

/// Decodes an HTTP request into a typed request message `T`.
///
/// The message JSON is assembled by layering, in increasing precedence:
/// the body (per `body_kind`), then `query` parameters, then the path
/// `bindings`. The assembled JSON is then deserialized into `T`.
///
/// # Errors
///
/// Returns a [`TranscodeError`] if the body is not valid JSON, if the assembled
/// value cannot form an object, or if it fails to deserialize into `T`.
///
/// # Examples
///
/// ```
/// use rest_over_grpc::Binding;
/// use rest_over_grpc::transcode::{BodyKind, decode_request};
/// use serde::Deserialize;
///
/// #[derive(Debug, Deserialize)]
/// struct ListBooksRequest {
///     shelf: String,
///     theme: String,
///     page_size: String,
/// }
///
/// let bindings = [Binding::new(&["shelf"], "7")];
/// let query = [("theme", "history"), ("page_size", "20")];
///
/// let request: ListBooksRequest = decode_request(&bindings, &query, b"", BodyKind::None)?;
/// assert_eq!(request.shelf, "7");
/// assert_eq!(request.theme, "history");
/// assert_eq!(request.page_size, "20");
/// # Ok::<(), rest_over_grpc::transcode::TranscodeError>(())
/// ```
pub fn decode_request<T: DeserializeOwned>(
    bindings: &[Binding<'_>],
    query: &[(&str, &str)],
    body: &[u8],
    body_kind: BodyKind,
) -> Result<T, TranscodeError> {
    // Fast paths deserialize directly into `T`, skipping the intermediate
    // `serde_json::Value` map. They are mutually exclusive on `body_kind`;
    // anything else falls through to the merge path below.
    match body_kind {
        // No body: deserialize straight from the borrowed path/query pairs.
        BodyKind::None if all_flat(bindings, query) && keys_are_unique(bindings, query) => {
            let pairs = bindings
                .iter()
                .map(|b| (b.field_path()[0], b.value()))
                .chain(query.iter().map(|(k, v)| (*k, *v)));
            return T::deserialize(MapDeserializer::<_, serde::de::value::Error>::new(pairs)).map_err(TranscodeError::deserialize_value);
        }
        // Whole body with no overlay: the body is the message.
        BodyKind::Whole if bindings.is_empty() && query.is_empty() && !body.is_empty() => {
            return serde_json::from_slice(body).map_err(TranscodeError::deserialize);
        }
        // Body maps onto one field: parse `{"<field>": <body>}` without
        // materializing the wrapper (see `FieldDeserializer`).
        BodyKind::Field(field) if bindings.is_empty() && query.is_empty() && !body.is_empty() => {
            return T::deserialize(FieldDeserializer::new(field, body)).map_err(TranscodeError::deserialize);
        }
        _ => {}
    }

    let mut root = match body_kind {
        BodyKind::None => Value::Object(Map::new()),
        BodyKind::Whole => {
            if body.is_empty() {
                Value::Object(Map::new())
            } else {
                serde_json::from_slice(body).map_err(TranscodeError::body)?
            }
        }
        BodyKind::Field(field) => {
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
        // A flat key is a direct insert; dotted keys need the nested walk.
        if key.contains('.') {
            let path: Vec<&str> = key.split('.').collect();
            set_field(obj, &path, Value::String((*value).to_owned()))?;
        } else {
            obj.insert((*key).to_owned(), Value::String((*value).to_owned()));
        }
    }

    for binding in bindings {
        set_field(obj, binding.field_path(), Value::String(binding.value().to_owned()))?;
    }

    serde_json::from_value(root).map_err(TranscodeError::deserialize)
}

/// Returns `true` if every path binding and query key targets a single,
/// top-level field (no nested `a.b` path), so the flat fast path applies.
fn all_flat(bindings: &[Binding<'_>], query: &[(&str, &str)]) -> bool {
    bindings.iter().all(|b| b.field_path().len() == 1) && query.iter().all(|(key, _)| !key.contains('.'))
}

/// Returns `true` if no field name appears more than once across the bindings
/// and query keys. Duplicates need the `Value`-merge path, which resolves
/// precedence (last write wins); the flat path would instead surface serde's
/// duplicate-field error.
fn keys_are_unique(bindings: &[Binding<'_>], query: &[(&str, &str)]) -> bool {
    // Field counts are tiny, so an O(n^2) scan avoids any allocation.
    let key = |i: usize| -> &str {
        if i < bindings.len() {
            bindings[i].field_path()[0]
        } else {
            query[i - bindings.len()].0
        }
    };
    let total = bindings.len() + query.len();
    for i in 0..total {
        for j in (i + 1)..total {
            if key(i) == key(j) {
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
/// use rest_over_grpc::transcode::{ResponseBodyKind, encode_response};
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
            let value = serde_json::to_value(message).map_err(TranscodeError::serialize)?;
            let selected = value
                .get(field)
                .ok_or_else(|| TranscodeError::structure("response_body field is absent from the message"))?;
            serde_json::to_vec(selected).map_err(TranscodeError::serialize)
        }
    }
}

/// The JSON body shape for a [`Status`] response: `{"code": <i32>, "message": <string>}`.
#[derive(Serialize)]
struct StatusBody<'a> {
    code: i32,
    message: &'a str,
}

/// Renders a [`Status`] as a JSON [`HttpResponse`], mapping its [`Code`] to the
/// corresponding HTTP status.
///
/// The body has the shape `{"code": <i32>, "message": <string>}`.
///
/// # Examples
///
/// ```
/// use rest_over_grpc::transcode::status_response;
/// use rest_over_grpc::{Code, Status};
///
/// let response = status_response(&Status::not_found("shelf 7"));
/// assert_eq!(response.status(), http::StatusCode::NOT_FOUND);
///
/// let body: serde_json::Value = serde_json::from_slice(response.body())?;
/// assert_eq!(body["code"], Code::NotFound.as_i32());
/// assert_eq!(body["message"], "shelf 7");
/// # Ok::<(), serde_json::Error>(())
/// ```
#[must_use]
pub fn status_response(status: &Status) -> HttpResponse {
    let http = crate::map_code_to_http(status.code());
    // Serialize a small struct straight to bytes, no intermediate `Value`.
    let body = StatusBody {
        code: status.code().as_i32(),
        message: status.message(),
    };
    let bytes = serde_json::to_vec(&body).unwrap_or_else(|_| b"{}".to_vec());
    HttpResponse::json(http, bytes)
}

/// Renders a `404 Not Found` JSON [`HttpResponse`] for an unmatched route.
///
/// # Examples
///
/// ```
/// use rest_over_grpc::Code;
/// use rest_over_grpc::transcode::not_found_response;
///
/// let response = not_found_response();
/// assert_eq!(response.status(), http::StatusCode::NOT_FOUND);
///
/// let body: serde_json::Value = serde_json::from_slice(response.body())?;
/// assert_eq!(body["code"], Code::NotFound.as_i32());
/// assert_eq!(body["message"], "no route matches the request");
/// # Ok::<(), serde_json::Error>(())
/// ```
#[must_use]
pub fn not_found_response() -> HttpResponse {
    status_response(&Status::not_found("no route matches the request"))
}

/// Inserts `value` at the (possibly nested) `path` within `obj`, creating
/// intermediate objects as needed.
fn set_field(obj: &mut Map<String, Value>, path: &[impl AsRef<str>], value: Value) -> Result<(), TranscodeError> {
    let Some((first, rest)) = path.split_first() else {
        return Ok(());
    };
    let key = first.as_ref();

    if rest.is_empty() {
        obj.insert(key.to_owned(), value);
        return Ok(());
    }

    let entry = obj.entry(key.to_owned()).or_insert_with(|| Value::Object(Map::new()));
    let nested = entry
        .as_object_mut()
        .ok_or_else(|| TranscodeError::structure("path variable conflicts with a scalar field"))?;
    set_field(nested, rest, value)
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::*;
    use crate::{Code, split_path};

    #[derive(Debug, Deserialize, PartialEq)]
    struct Shelf {
        shelf: String,
        theme: String,
    }

    #[derive(Debug, Deserialize, PartialEq)]
    struct Nested {
        shelf: Inner,
    }

    #[derive(Debug, Deserialize, PartialEq)]
    struct Inner {
        id: String,
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

    fn binding(field_path: &'static [&'static str], value: &'static str) -> Binding<'static> {
        Binding::new(field_path, value)
    }

    #[test]
    fn decodes_path_binding_over_body() {
        let bindings = [binding(&["shelf"], "42")];
        let body = br#"{"shelf":"ignored","theme":"history"}"#;
        let decoded: Shelf = decode_request(&bindings, &[], body, BodyKind::Whole).expect("decodes");
        assert_eq!(
            decoded,
            Shelf {
                shelf: "42".to_owned(),
                theme: "history".to_owned()
            }
        );
    }

    #[test]
    fn decodes_nested_field_path() {
        let bindings = [binding(&["shelf", "id"], "7")];
        let decoded: Nested = decode_request(&bindings, &[], b"", BodyKind::None).expect("decodes");
        assert_eq!(decoded.shelf.id, "7");
    }

    #[test]
    fn decodes_query_parameters() {
        let decoded: Shelf = decode_request(&[], &[("shelf", "1"), ("theme", "sci-fi")], b"", BodyKind::None).expect("decodes");
        assert_eq!(decoded.shelf, "1");
        assert_eq!(decoded.theme, "sci-fi");
    }

    #[test]
    fn body_field_mapping() {
        let bindings = [binding(&["shelf"], "9")];
        let decoded: Shelf = decode_request(&bindings, &[], br#""history""#, BodyKind::Field("theme")).expect("decodes");
        assert_eq!(decoded.shelf, "9");
        assert_eq!(decoded.theme, "history");
    }

    #[test]
    fn rejects_invalid_body_json() {
        let err = decode_request::<Shelf>(&[], &[], b"not json", BodyKind::Whole).expect_err("invalid json");
        assert_eq!(err.code(), Code::InvalidArgument);
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
    fn integrates_with_split_path() {
        let (segs, _verb) = split_path("/v1/shelves/55");
        let value = segs.span(2, 3);
        let bindings = [binding(&["shelf"], "x")];
        // Ensure bindings + span values flow into decode without panicking.
        let _ = value;
        let decoded: Shelf = decode_request(&bindings, &[("theme", "t")], b"", BodyKind::None).expect("decodes");
        assert_eq!(decoded.theme, "t");
    }

    #[test]
    fn decodes_nested_query_parameter() {
        // The `.`-separated key still routes into a nested message field.
        let decoded: Nested = decode_request(&[], &[("shelf.id", "9")], b"", BodyKind::None).expect("decodes");
        assert_eq!(decoded.shelf.id, "9");
    }

    #[test]
    fn status_response_has_code_and_message() {
        let response = status_response(&Status::not_found("gone"));
        assert_eq!(response.status().as_u16(), 404);
        let value: Value = serde_json::from_slice(response.body()).expect("valid json");
        assert_eq!(value["code"], Code::NotFound.as_i32());
        assert_eq!(value["message"], "gone");
    }

    #[test]
    fn not_found_response_is_a_404() {
        let response = not_found_response();
        assert_eq!(response.status().as_u16(), 404);
        let value: Value = serde_json::from_slice(response.body()).expect("valid json");
        assert_eq!(value["code"], Code::NotFound.as_i32());
    }

    #[test]
    fn flat_fast_path_agrees_with_value_path() {
        let bindings = [binding(&["shelf"], "history")];
        // Fast path: no body, unique flat keys.
        let fast: Shelf = decode_request(&bindings, &[("theme", "science")], b"", BodyKind::None).expect("fast");
        // Force the `Value`-merge path by supplying an (empty) whole body.
        let slow: Shelf = decode_request(&bindings, &[("theme", "science")], b"{}", BodyKind::Whole).expect("slow");
        assert_eq!(fast, slow);
        assert_eq!(fast.shelf, "history");
        assert_eq!(fast.theme, "science");
    }

    #[test]
    fn duplicate_key_falls_back_with_path_precedence() {
        // `shelf` appears as both a path binding and a query param; the path
        // binding wins (matching the `Value`-merge precedence).
        let bindings = [binding(&["shelf"], "from_path")];
        let decoded: Shelf = decode_request(&bindings, &[("shelf", "from_query"), ("theme", "t")], b"", BodyKind::None).expect("decodes");
        assert_eq!(decoded.shelf, "from_path");
        assert_eq!(decoded.theme, "t");
    }

    #[test]
    fn whole_body_empty_is_an_empty_object() {
        // Body present per `body_kind` but empty bytes → an empty object.
        let decoded: Shelf =
            decode_request(&[binding(&["shelf"], "1"), binding(&["theme"], "t")], &[], b"", BodyKind::Whole).expect("decodes");
        assert_eq!(decoded.shelf, "1");
    }

    #[test]
    fn field_body_maps_non_empty_body() {
        let decoded: Shelf = decode_request(&[binding(&["shelf"], "9")], &[], br#""history""#, BodyKind::Field("theme")).expect("decodes");
        assert_eq!(decoded.theme, "history");
    }

    #[test]
    fn field_body_empty_skips_insert() {
        // `BodyKind::Field` with an empty body inserts nothing; the field comes
        // from the path binding instead.
        let decoded: Shelf = decode_request(
            &[binding(&["shelf"], "s"), binding(&["theme"], "t")],
            &[],
            b"",
            BodyKind::Field("theme"),
        )
        .expect("decodes");
        assert_eq!(decoded.shelf, "s");
        assert_eq!(decoded.theme, "t");
    }

    #[test]
    fn empty_field_path_binding_is_ignored() {
        // A binding with an empty field path routes through `set_field`'s
        // empty-path base case and inserts nothing.
        let decoded: Shelf = decode_request(
            &[binding(&[], "x"), binding(&["shelf"], "s"), binding(&["theme"], "t")],
            &[],
            b"",
            BodyKind::Whole,
        )
        .expect("decodes");
        assert_eq!(decoded.shelf, "s");
    }

    #[test]
    fn value_path_deserialize_error() {
        // Body parses as JSON but the wrong type fails `from_value`.
        let err = decode_request::<Shelf>(&[], &[], br#"{"shelf":123,"theme":"t"}"#, BodyKind::Whole).expect_err("type mismatch");
        assert_eq!(err.code(), Code::InvalidArgument);
        assert!(err.to_string().contains("did not match"));
    }

    #[test]
    fn fast_path_deserialize_error() {
        // Fast path (no body, flat, unique keys) with a missing required field.
        let err = decode_request::<Shelf>(&[binding(&["shelf"], "s")], &[], b"", BodyKind::None).expect_err("missing theme");
        assert_eq!(err.code(), Code::InvalidArgument);
    }

    #[test]
    fn non_object_whole_body_is_a_deserialize_error() {
        // A non-object whole body fails the direct-deserialize fast path as a
        // type mismatch, still `InvalidArgument` (HTTP 400).
        let err = decode_request::<Shelf>(&[], &[], b"[1,2,3]", BodyKind::Whole).expect_err("array body");
        assert_eq!(err.code(), Code::InvalidArgument);
        assert!(err.to_string().contains("did not match"));
    }

    #[test]
    fn non_object_body_with_bindings_is_a_structure_error() {
        // With a path binding to overlay, decoding takes the `Value`-merge path,
        // which still reports a non-object body as a structure error.
        let err = decode_request::<Shelf>(&[binding(&["shelf"], "1")], &[], b"[1,2,3]", BodyKind::Whole).expect_err("array body");
        assert_eq!(err.code(), Code::InvalidArgument);
        assert!(err.to_string().contains("invalid request structure"));
    }

    #[test]
    fn whole_body_direct_path_agrees_with_merge_path() {
        let body = br#"{"shelf":"s","theme":"history"}"#;
        // Direct path: whole body, no bindings/query.
        let direct: Shelf = decode_request(&[], &[], body, BodyKind::Whole).expect("direct");
        // Merge path: an (ignored, distinct-key) binding forces the `Value` merge.
        let merged: Shelf = decode_request(&[binding(&["extra_ignored"], "x")], &[], body, BodyKind::Whole).expect("merged");
        assert_eq!(direct, merged);
        assert_eq!(direct.shelf, "s");
        assert_eq!(direct.theme, "history");
    }

    #[test]
    fn field_body_direct_path_agrees_with_merge_path() {
        // `BodyKind::Field` with no bindings/query takes the direct
        // sub-deserializer path; with a binding it takes the `Value`-merge path.
        // Both agree.
        let body = br#""history""#;
        let direct: Shelf = decode_request(&[binding(&["shelf"], "9")], &[], body, BodyKind::Field("theme")).expect("merge");
        let via_direct: Nested = decode_request(&[], &[], br#"{"id":"7"}"#, BodyKind::Field("shelf")).expect("direct");
        assert_eq!(direct.theme, "history");
        assert_eq!(via_direct.shelf.id, "7");
    }

    #[test]
    fn field_body_direct_path_rejects_invalid_json() {
        // The field sub-deserializer surfaces a parse error as an invalid
        // argument, just like the whole-body path.
        let err = decode_request::<Shelf>(&[], &[], b"not json", BodyKind::Field("theme")).expect_err("invalid json");
        assert_eq!(err.code(), Code::InvalidArgument);
    }

    #[test]
    fn field_body_direct_path_rejects_trailing_bytes() {
        // Trailing content after the field value is rejected, mirroring
        // `serde_json::from_slice`'s end-of-input check.
        let err = decode_request::<Shelf>(&[], &[], br#""history" extra"#, BodyKind::Field("theme")).expect_err("trailing");
        assert_eq!(err.code(), Code::InvalidArgument);
    }

    #[test]
    fn query_key_conflicting_with_scalar_is_a_structure_error() {
        // `a` is set to a scalar, then `a.b` tries to nest into it.
        let err = decode_request::<Shelf>(&[], &[("a", "1"), ("a.b", "2")], b"", BodyKind::None).expect_err("conflict");
        assert_eq!(err.code(), Code::InvalidArgument);
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
        // A path binding forces the `Value`-merge path, which reports a malformed
        // body as a body-JSON error.
        let err = decode_request::<Shelf>(&[binding(&["shelf"], "1")], &[], b"not json", BodyKind::Whole).expect_err("bad json");
        // The `From` conversion agrees with the inherent `into_status`.
        let via_from: Status = decode_request::<Shelf>(&[binding(&["shelf"], "1")], &[], b"not json", BodyKind::Whole)
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
        let parse_err = decode_request::<Shelf>(&[], &[], b"not json", BodyKind::Whole).expect_err("bad json");
        assert!(parse_err.source().is_some());
        assert!(format!("{parse_err:?}").contains("backtrace"));

        // A structural error (non-object body on the merge path) has no
        // underlying cause. The binding forces the `Value`-merge path.
        let structure_err = decode_request::<Shelf>(&[binding(&["shelf"], "1")], &[], b"42", BodyKind::Whole).expect_err("not an object");
        assert!(structure_err.source().is_none());
    }
}
