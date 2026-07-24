// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Serde-based JSON-to-message request and response transcoding.
//!
//! Query and body fields deserialize through proto3 JSON-compatible serde
//! implementations. Generated code assigns captured path fields directly.

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
pub use rest_parse::{RestParse, parse_path_enum_value, parse_path_field, parse_reserved_path_enum_value, parse_reserved_path_field};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde::de::value::MapDeserializer;
use serde_json::{from_slice, to_value, to_writer};

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

    if let Some(result) = overlay::try_decode_overlay(&body_kind, query, body) {
        return result;
    }

    debug_assert!(matches!(body_kind, RequestBodyKind::Field(_)) && query.is_empty() && body.is_empty());
    from_slice(b"{}").map_err(TranscodeError::deserialize)
}

/// Deserializes directly when no body/query merge is needed.
#[cfg_attr(test, mutants::skip)]
fn try_decode_fast<T: DeserializeOwned>(
    body_kind: &RequestBodyKind,
    query: &[(&str, &str)],
    body: &[u8],
) -> Option<Result<T, TranscodeError>> {
    match body_kind {
        RequestBodyKind::None if query.is_empty() => {
            let pairs = query.iter().map(|(k, v)| (*k, *v));
            Some(T::deserialize(MapDeserializer::<_, serde::de::value::Error>::new(pairs)).map_err(TranscodeError::deserialize_value))
        }
        RequestBodyKind::Whole if query.is_empty() && !body.is_empty() => {
            Some(from_slice(body).map_err(TranscodeError::body_or_deserialize))
        }
        RequestBodyKind::Field(field) if query.is_empty() && !body.is_empty() => {
            Some(T::deserialize(FieldDeserializer::new(field, body)).map_err(TranscodeError::deserialize))
        }
        _ => None,
    }
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
    let mut body = Vec::new();
    encode_response_into(message, kind, &mut body)?;
    Ok(body)
}

pub(crate) fn encode_response_into<T: Serialize>(message: &T, kind: ResponseBodyKind, body: &mut Vec<u8>) -> Result<(), TranscodeError> {
    match kind {
        ResponseBodyKind::Whole => to_writer(body, message).map_err(TranscodeError::serialize),
        ResponseBodyKind::Field(field) => match message.serialize(FieldSerializer::new(field, body)) {
            Ok(()) => Ok(()),
            Err(FieldSerError::Absent) => Err(TranscodeError::response_structure("response_body field is absent from the message")),
            Err(FieldSerError::Unsupported) => encode_field_via_value_into(message, field, body),
            Err(FieldSerError::Json(source)) => Err(TranscodeError::serialize(source)),
            Err(FieldSerError::Custom(detail)) => Err(TranscodeError::serialize_message(detail)),
        },
    }
}

/// Handles non-struct response shapes through a JSON value.
fn encode_field_via_value_into<T: Serialize>(message: &T, field: &str, body: &mut Vec<u8>) -> Result<(), TranscodeError> {
    let value = to_value(message).map_err(TranscodeError::serialize)?;
    let selected = value
        .get(field)
        .ok_or_else(|| TranscodeError::response_structure("response_body field is absent from the message"))?;
    to_writer(body, selected).map_err(TranscodeError::serialize)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde::Deserialize;
    use serde_json::Value;

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

    struct Unserializable;

    impl Serialize for Unserializable {
        fn serialize<S: serde::Serializer>(&self, _serializer: S) -> Result<S::Ok, S::Error> {
            Err(serde::ser::Error::custom("intentional serialize failure"))
        }
    }

    #[test]
    fn decodes_query_parameters() {
        let decoded: Shelf = decode_request(&[("shelf", "1"), ("theme", "sci-fi")], b"", RequestBodyKind::None).expect("decodes");
        assert_eq!(decoded.shelf, "1");
        assert_eq!(decoded.theme, "sci-fi");
    }

    #[test]
    fn query_overlays_body() {
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
        assert!(err.to_string().contains("invalid request body JSON"));
    }

    #[test]
    fn whole_body_reaches_the_value_merge_path_via_a_dotted_query() {
        let non_empty: Value =
            decode_request(&[("nested.key", "v")], br#"{"shelf":"7"}"#, RequestBodyKind::Whole).expect("non-empty whole body decodes");
        assert_eq!(non_empty["shelf"], "7");
        assert_eq!(non_empty["nested"]["key"], "v");

        let empty: Value = decode_request(&[("nested.key", "v")], b"", RequestBodyKind::Whole).expect("empty whole body decodes");
        assert_eq!(empty["nested"]["key"], "v");
        assert!(empty.get("shelf").is_none());
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
    fn response_encoding_appends_to_an_existing_buffer() {
        let resp = Resp {
            name: "shelves/1".to_owned(),
            size: 3,
        };
        let mut body = b"prefix:".to_vec();

        encode_response_into(&resp, ResponseBodyKind::Field("name"), &mut body).expect("encodes");

        assert_eq!(body, br#"prefix:"shelves/1""#);
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
        let resp = Some(Resp {
            name: "wrapped".to_owned(),
            size: 1,
        });
        let bytes = encode_response(&resp, ResponseBodyKind::Field("name")).expect("encodes");
        assert_eq!(bytes, br#""wrapped""#);
    }

    #[test]
    fn field_selection_ignores_unserializable_sibling() {
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
        let err = encode_response(&Unserializable, ResponseBodyKind::Field("any")).expect_err("top-level serialize fails");
        assert_eq!(err.code(), Code::Internal);
        assert!(err.to_string().contains("serialize"));
    }

    #[test]
    fn field_selection_on_non_object_is_absent() {
        let err = encode_response(&"scalar", ResponseBodyKind::Field("name")).expect_err("no fields");
        assert_eq!(err.code(), Code::Internal);
        assert!(err.to_string().starts_with("failed to encode the response:"));
    }

    #[test]
    fn field_selection_falls_back_for_map_messages() {
        let mut map = BTreeMap::new();
        let _ = map.insert("name".to_owned(), "from_map".to_owned());
        let _ = map.insert("theme".to_owned(), "history".to_owned());
        let bytes = encode_response(&map, ResponseBodyKind::Field("name")).expect("encodes via fallback");
        assert_eq!(bytes, br#""from_map""#);
    }

    #[test]
    fn field_selection_map_fallback_reports_absent() {
        let mut map = BTreeMap::new();
        let _ = map.insert("name".to_owned(), "x".to_owned());
        let err = encode_response(&map, ResponseBodyKind::Field("missing")).expect_err("absent");
        assert_eq!(err.code(), Code::Internal);
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
        let decoded: Shelf = decode_request(&[("shelf", "55"), ("theme", "t")], b"", RequestBodyKind::None).expect("decodes");
        assert_eq!(decoded.shelf, "55");
        assert_eq!(decoded.theme, "t");
    }

    #[test]
    fn decodes_nested_query_parameter() {
        let decoded: Nested = decode_request(&[("shelf.id", "9")], b"", RequestBodyKind::None).expect("decodes");
        assert_eq!(decoded.shelf.id, "9");
    }

    #[test]
    fn flat_fast_path_agrees_with_value_path() {
        let fast: Shelf = decode_request(&[("shelf", "history"), ("theme", "science")], b"", RequestBodyKind::None).expect("fast");
        let slow: Shelf = decode_request(&[("shelf", "history"), ("theme", "science")], b"{}", RequestBodyKind::Whole).expect("slow");
        assert_eq!(fast, slow);
        assert_eq!(fast.shelf, "history");
        assert_eq!(fast.theme, "science");
    }

    #[test]
    fn query_shadows_body_field() {
        let body = br#"{"shelf":"from_body","theme":"t"}"#;
        let decoded: Shelf = decode_request(&[("shelf", "from_query")], body, RequestBodyKind::Whole).expect("decodes");
        assert_eq!(decoded.shelf, "from_query");
        assert_eq!(decoded.theme, "t");
    }

    #[test]
    fn whole_body_empty_is_an_empty_object() {
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
        let decoded: Shelf = decode_request(&[("shelf", "s"), ("theme", "t")], b"", RequestBodyKind::Field("theme")).expect("decodes");
        assert_eq!(decoded.shelf, "s");
        assert_eq!(decoded.theme, "t");
    }

    #[test]
    fn empty_requests_cover_direct_and_empty_field_paths() {
        #[derive(Debug, Deserialize, PartialEq)]
        #[expect(clippy::empty_structs_with_brackets, reason = "represents an empty JSON object")]
        struct Empty {}

        assert_eq!(
            decode_request::<Empty>(&[], b"", RequestBodyKind::None).expect("empty request"),
            Empty {}
        );
        assert_eq!(
            decode_request::<Empty>(&[], b"", RequestBodyKind::Field("payload")).expect("empty field request"),
            Empty {}
        );
        let _ = decode_request::<Shelf>(&[], b"", RequestBodyKind::None).expect_err("required fields are absent");
    }

    #[test]
    fn value_path_deserialize_error() {
        let err = decode_request::<Shelf>(&[], br#"{"shelf":123,"theme":"t"}"#, RequestBodyKind::Whole).expect_err("type mismatch");
        assert_eq!(err.code(), Code::InvalidArgument);
        assert!(err.to_string().contains("did not match"));
    }

    #[test]
    fn fast_path_deserialize_error() {
        let err = decode_request::<Shelf>(&[("shelf", "s")], b"", RequestBodyKind::None).expect_err("missing theme");
        assert_eq!(err.code(), Code::InvalidArgument);
    }

    #[test]
    fn non_object_whole_body_is_a_deserialize_error() {
        let err = decode_request::<Shelf>(&[], b"[1,2,3]", RequestBodyKind::Whole).expect_err("array body");
        assert_eq!(err.code(), Code::InvalidArgument);
        assert!(err.to_string().contains("did not match"));
    }

    #[test]
    fn non_object_body_with_query_is_a_structure_error() {
        let err = decode_request::<Shelf>(&[("shelf", "1")], b"[1,2,3]", RequestBodyKind::Whole).expect_err("array body");
        assert_eq!(err.code(), Code::InvalidArgument);
        assert!(err.to_string().contains("invalid request structure"));
    }

    #[test]
    fn whole_body_direct_path_agrees_with_merge_path() {
        let body = br#"{"shelf":"s","theme":"history"}"#;
        let direct: Shelf = decode_request(&[], body, RequestBodyKind::Whole).expect("direct");
        let merged: Shelf = decode_request(&[("extra_ignored", "x")], body, RequestBodyKind::Whole).expect("merged");
        assert_eq!(direct, merged);
        assert_eq!(direct.shelf, "s");
        assert_eq!(direct.theme, "history");
    }

    #[test]
    fn field_body_direct_path_agrees_with_merge_path() {
        let direct: Shelf = decode_request(&[("shelf", "9")], br#""history""#, RequestBodyKind::Field("theme")).expect("merge");
        let via_direct: Nested = decode_request(&[], br#"{"id":"7"}"#, RequestBodyKind::Field("shelf")).expect("direct");
        assert_eq!(direct.theme, "history");
        assert_eq!(via_direct.shelf.id, "7");
    }

    #[test]
    fn field_body_direct_path_rejects_invalid_json() {
        let err = decode_request::<Shelf>(&[], b"not json", RequestBodyKind::Field("theme")).expect_err("invalid json");
        assert_eq!(err.code(), Code::InvalidArgument);
    }

    #[test]
    fn overlay_path_preserves_nested_body_and_applies_precedence() {
        #[derive(Debug, Deserialize, PartialEq)]
        struct Outer {
            shelf: String,
            theme: String,
            inner: Inner,
        }

        let body = br#"{"shelf":"from-body","inner":{"id":"nested"}}"#;
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
        let decoded: Shelf =
            decode_request(&[("shelf", "a%2Fb"), ("theme", "science%20fiction")], b"", RequestBodyKind::None).expect("overlay decodes");
        assert_eq!(decoded.shelf, "a/b");
        assert_eq!(decoded.theme, "science fiction");
    }

    #[test]
    fn field_body_direct_path_rejects_trailing_bytes() {
        let err = decode_request::<Shelf>(&[], br#""history" extra"#, RequestBodyKind::Field("theme")).expect_err("trailing");
        assert_eq!(err.code(), Code::InvalidArgument);
    }

    #[test]
    fn query_key_conflicting_with_scalar_is_a_structure_error() {
        let err = decode_request::<Shelf>(&[("a", "1"), ("a.b", "2")], b"", RequestBodyKind::None).expect_err("conflict");
        assert_eq!(err.code(), Code::InvalidArgument);
    }

    #[test]
    fn percent_decodes_query_value_and_plus() {
        let decoded: Shelf =
            decode_request(&[("shelf", "7"), ("theme", "science+%66iction")], b"", RequestBodyKind::None).expect("decodes");
        assert_eq!(decoded.shelf, "7");
        assert_eq!(decoded.theme, "science fiction");
    }

    #[test]
    fn percent_decodes_query_names_and_rejects_invalid_encoding() {
        let decoded: Nested = decode_request(&[("shelf%2Eid", "9")], b"", RequestBodyKind::None).expect("encoded name decodes");
        assert_eq!(decoded.shelf.id, "9");

        let _ = decode_request::<Shelf>(&[("shelf", "%FF"), ("theme", "x")], b"", RequestBodyKind::None).expect_err("invalid UTF-8");
        let _ = decode_request::<Shelf>(&[("shelf%zz", "x"), ("theme", "x")], b"", RequestBodyKind::None).expect_err("malformed escape");
    }

    #[test]
    fn repeated_query_parameters_form_an_array() {
        #[derive(Debug, Deserialize)]
        struct Repeated {
            tag: Vec<String>,
        }

        let decoded: Repeated = decode_request(&[("tag", "a"), ("tag", "b")], b"", RequestBodyKind::None).expect("repeated field");
        assert_eq!(decoded.tag, ["a", "b"]);

        let scalar = decode_request::<BTreeMap<String, String>>(&[("tag", "a"), ("tag", "b")], b"", RequestBodyKind::None);
        assert!(scalar.is_err(), "duplicate scalar parameters must not silently discard a value");
    }

    #[test]
    fn query_parameters_decode_bool_single_repeated_and_numeric_enum() {
        #[derive(Debug, Deserialize, PartialEq)]
        struct Query {
            enabled: bool,
            tags: Vec<String>,
            genre: Genre,
            string_number: String,
        }

        #[derive(Debug, PartialEq)]
        enum Genre {
            Science,
        }

        impl<'de> Deserialize<'de> for Genre {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                struct GenreVisitor;

                impl serde::de::Visitor<'_> for GenreVisitor {
                    type Value = Genre;

                    #[cfg_attr(coverage_nightly, coverage(off))]
                    fn expecting(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                        formatter.write_str("SCIENCE or 2")
                    }

                    fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Self::Value, E> {
                        (v == 2).then_some(Genre::Science).ok_or_else(|| E::custom("unknown genre number"))
                    }

                    fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
                        (v == "SCIENCE")
                            .then_some(Genre::Science)
                            .ok_or_else(|| E::custom("unknown genre name"))
                    }
                }

                deserializer.deserialize_any(GenreVisitor)
            }
        }

        let decoded: Query = decode_request(
            &[("enabled", "true"), ("tags", "one"), ("genre", "2"), ("string_number", "2")],
            b"",
            RequestBodyKind::None,
        )
        .expect("typed query values decode");
        assert_eq!(
            decoded,
            Query {
                enabled: true,
                tags: vec!["one".to_owned()],
                genre: Genre::Science,
                string_number: "2".to_owned(),
            }
        );

        let named: Query = decode_request(
            &[("enabled", "true"), ("tags", "one"), ("genre", "SCIENCE"), ("string_number", "2")],
            b"",
            RequestBodyKind::None,
        )
        .expect("enum name decodes");
        assert_eq!(named.genre, Genre::Science);
        let _ = decode_request::<Query>(
            &[("enabled", "true"), ("tags", "one"), ("genre", "BOGUS"), ("string_number", "2")],
            b"",
            RequestBodyKind::None,
        )
        .expect_err("unknown enum name");
    }

    #[test]
    fn nested_and_body_mapped_query_parameters_use_typed_decoding() {
        #[derive(Debug, Deserialize, PartialEq)]
        struct Outer {
            nested: NestedQuery,
        }
        #[derive(Debug, Deserialize, PartialEq)]
        struct NestedQuery {
            enabled: bool,
            tags: Vec<String>,
            keep: String,
        }
        #[derive(Debug, Deserialize, PartialEq)]
        struct FieldBody {
            payload: String,
            enabled: bool,
        }
        let decoded: Outer = decode_request(
            &[("nested.enabled", "true"), ("nested.tags", "a"), ("nested.tags", "b")],
            br#"{"nested":{"enabled":false,"keep":"body"}}"#,
            RequestBodyKind::Whole,
        )
        .expect("nested query overlay");
        assert_eq!(
            decoded,
            Outer {
                nested: NestedQuery {
                    enabled: true,
                    tags: vec!["a".to_owned(), "b".to_owned()],
                    keep: "body".to_owned(),
                },
            }
        );

        let decoded: FieldBody = decode_request(&[("enabled", "true")], br#""content""#, RequestBodyKind::Field("payload"))
            .expect("field body with typed query");
        assert_eq!(
            decoded,
            FieldBody {
                payload: "content".to_owned(),
                enabled: true,
            }
        );
    }

    #[test]
    fn undecoded_values_still_take_the_fast_path() {
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
        assert_eq!(err.code(), Code::Internal);
        assert!(err.into_status().message().starts_with("failed to encode the response:"));
    }

    #[test]
    fn error_into_status_carries_code_and_message() {
        let err = decode_request::<Shelf>(&[("shelf", "1")], b"not json", RequestBodyKind::Whole).expect_err("bad json");
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

        let parse_err = decode_request::<Shelf>(&[], b"not json", RequestBodyKind::Whole).expect_err("bad json");
        assert!(parse_err.source().is_some());
        assert!(format!("{parse_err:?}").contains("backtrace"));

        let structure_err = decode_request::<Shelf>(&[("shelf", "1")], b"42", RequestBodyKind::Whole).expect_err("not an object");
        assert!(structure_err.source().is_none());
    }
}
