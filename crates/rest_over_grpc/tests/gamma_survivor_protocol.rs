// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! In-memory protocol, overlay, and streaming regression oracles.

use std::collections::BTreeMap;

use futures::executor::block_on;
use futures::{StreamExt, stream};
use http::{HeaderMap, HeaderValue, StatusCode, header};
use rest_over_grpc::codegen_helpers::{
    QueryPairs, RequestBodyKind, ResponseBodyKind, decode_request, encode_frames, encode_response, parse_path_enum_value, parse_path_field,
    parse_query, parse_reserved_path_enum_value, parse_reserved_path_field, split_query,
};
use rest_over_grpc::handling::{Code, Context, Status};
use rest_over_grpc::transcoding::{HttpResponse, StreamEncoding, StreamingResponse};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[test]
fn gamma_round2_reserved_path_keeps_literal_plus_and_encoded_slash() {
    for (input, expected) in [("a+b", "a+b"), ("a+%2Fb%2f+%2B", "a+%2Fb%2f++")] {
        assert_eq!(parse_reserved_path_field::<String>(input).unwrap(), expected);
    }
}

#[test]
fn gamma_round2_nested_query_invalid_value_identifies_value_not_key() {
    let error = decode_request::<Value>(&[("nested.value", "%GG")], b"", RequestBodyKind::None).unwrap_err();
    assert_eq!(error.code(), Code::InvalidArgument);
    assert!(error.to_string().contains("query parameter value"), "{error}");
}

#[test]
fn gamma_round2_accept_ignored_parameters_do_not_hide_later_quality() {
    for parameter in ["unknown", "charset=utf-8", "flag; charset=utf-8"] {
        let input = format!("text/event-stream; {parameter}; q=0");
        assert_eq!(StreamEncoding::from_accept(&input), StreamEncoding::JsonArray, "{input}");
    }
    for parameter in ["q", "q=0.1;q=0.2", "q=0.x", "q=0.+5", "q=2", "q=0.0001"] {
        assert_eq!(
            StreamEncoding::from_accept(&format!("text/event-stream;{parameter}")),
            StreamEncoding::JsonArray
        );
    }
}

#[test]
fn gamma_round2_query_flags_preserve_empty_values_and_pair_order() {
    let pairs = QueryPairs::parse_limited("flag&explicit=&next=7&flag&&").unwrap();
    assert_eq!(pairs.as_slice(), [("flag", ""), ("explicit", ""), ("next", "7"), ("flag", "")]);
}

#[test]
fn gamma_round2_stream_selected_field_serialization_failure_is_status_not_panic() {
    for encoding in [StreamEncoding::JsonArray, StreamEncoding::NdJson, StreamEncoding::Sse] {
        let response = StreamingResponse::encode_response(
            stream::iter([Ok::<_, Status>(Bad)]),
            encoding,
            ResponseBodyKind::Field {
                name: "value",
                default: "null",
            },
        );
        let frames = block_on(response.into_frames().collect::<Vec<_>>());
        assert_eq!(frames.len(), 1);
        let error = frames[0].as_ref().unwrap_err();
        assert_eq!(error.code(), Code::Internal);
        assert!(error.message().contains("deliberate serialization failure"));
    }
}

#[test]
fn gamma_survivor_accept_quality_grammar_order_and_multiline() {
    for (values, expected) in [
        (vec!["TEXT/EVENT-STREAM ; Q = 0.123"], StreamEncoding::Sse),
        (vec!["application/json;q=0.099,text/event-stream;q=0.1"], StreamEncoding::Sse),
        (
            vec!["application/json;q=0.999", "application/jsonl;q=1.000"],
            StreamEncoding::NdJson,
        ),
        (vec!["text/event-stream;q=1.,application/json;q=1"], StreamEncoding::Sse),
        (
            vec!["application/x-ndjson;q=0.001,text/event-stream;q=0.000"],
            StreamEncoding::NdJson,
        ),
        (vec!["application/jsonl;q=0.3,text/event-stream;q=0.3"], StreamEncoding::NdJson),
        (vec!["text/event-stream; charset=utf-8; ignored; q=1"], StreamEncoding::Sse),
        (vec!["text/event-stream;q=0", "application/x-ndjson;q=0"], StreamEncoding::JsonArray),
        (vec!["text/plain;q=1,text/event-stream;q=0.2"], StreamEncoding::Sse),
        (vec!["application/*;q=1,text/event-stream;q=0.9"], StreamEncoding::JsonArray),
        (vec!["text/event-stream;q=1,application/jsonl"], StreamEncoding::Sse),
        (vec!["application/jsonl,text/event-stream;q=1"], StreamEncoding::NdJson),
        (vec!["*/*;q=1,text/event-stream;q=0.9"], StreamEncoding::JsonArray),
    ] {
        assert_eq!(StreamEncoding::from_accept_values(values), expected);
    }
    for quality in ["", "q", "-1", "2", "01", ".1", "0.x", "0.0001", "1.001", "NaN", "0.1;q=0.2"] {
        let accept = format!("text/event-stream;q={quality},application/x-ndjson;q=0.001");
        assert_eq!(StreamEncoding::from_accept(&accept), StreamEncoding::NdJson, "{quality}");
    }
    assert_eq!(
        StreamEncoding::from_accept("text/event-stream;q,application/jsonl"),
        StreamEncoding::NdJson
    );
}

#[test]
fn gamma_survivor_frames_are_exact_and_fuse_after_errors() {
    for (encoding, frames) in [
        (StreamEncoding::JsonArray, vec![b"[1".to_vec(), b",2".to_vec(), b"]".to_vec()]),
        (StreamEncoding::NdJson, vec![b"1\n".to_vec(), b"2\n".to_vec()]),
        (StreamEncoding::Sse, vec![b"data: 1\n\n".to_vec(), b"data: 2\n\n".to_vec()]),
    ] {
        let first_frame = frames[0].clone();
        let actual = block_on(encode_frames(stream::iter([Ok::<_, Status>(1), Ok(2)]), encoding).collect::<Vec<_>>());
        assert_eq!(actual, frames.into_iter().map(Ok).collect::<Vec<_>>());
        let input = stream::iter([Ok(1), Err(Status::new(Code::Aborted, "mid-stream")), Ok(99)]);
        let actual = block_on(encode_frames(input, encoding).collect::<Vec<_>>());
        assert_eq!(actual.len(), 2);
        assert_eq!(actual[0], Ok(first_frame));
        assert_eq!(actual[1], Err(Status::new(Code::Aborted, "mid-stream")));
        let empty = block_on(encode_frames(stream::empty::<Result<u32, Status>>(), encoding).collect::<Vec<_>>());
        assert_eq!(
            empty,
            if encoding == StreamEncoding::JsonArray {
                vec![Ok(b"[]".to_vec())]
            } else {
                vec![]
            }
        );
    }
}

#[test]
fn gamma_survivor_stream_frame_capacity_clamps() {
    let frames = block_on(encode_frames(stream::iter([Ok::<_, Status>(1), Ok(2)]), StreamEncoding::NdJson).collect::<Vec<_>>());
    assert_eq!(frames[0].as_ref().unwrap().capacity(), 136);
    assert_eq!(frames[1].as_ref().unwrap().capacity(), 136);
    let frames = block_on(
        encode_frames(
            stream::iter([Ok::<_, Status>("x".repeat(70_000)), Ok("small".into()), Ok("last".into())]),
            StreamEncoding::NdJson,
        )
        .collect::<Vec<_>>(),
    );
    assert_eq!(frames[1].as_ref().unwrap().capacity(), 65_536);
    assert_eq!(frames[2].as_ref().unwrap().capacity(), 136);
    assert_eq!(frames[1].as_ref().unwrap().as_slice(), b"\"small\"\n");
}

struct Bad;
impl Serialize for Bad {
    fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
        Err(serde::ser::Error::custom("deliberate serialization failure"))
    }
}

#[test]
fn gamma_nov4_selected_map_field_returns_value_fallback_serialization_errors_without_panicking() {
    use std::collections::BTreeMap;

    let message = BTreeMap::from([("selected", (Bad, 1_u8))]);
    let error = encode_response(
        &message,
        ResponseBodyKind::Field {
            name: "selected",
            default: "null",
        },
    )
    .unwrap_err();
    assert_eq!(error.code(), Code::Internal);
    assert!(error.to_string().contains("deliberate serialization failure"), "{error}");
    assert!(
        std::error::Error::source(&error)
            .expect("serialization failures preserve their source")
            .is::<serde_json::Error>()
    );
}

#[test]
fn gamma_nov4_direct_flat_overlay_keeps_an_empty_json_object_key() {
    let query = [("", "37")];
    assert_eq!(
        decode_request::<Value>(&query, b"", RequestBodyKind::None).unwrap(),
        json!({"": 37})
    );
    assert_eq!(
        decode_request::<Value>(&query, br#"{"keep":11,"":5}"#, RequestBodyKind::Whole).unwrap(),
        json!({"keep": 11, "": 37})
    );
}

struct DuplicateTarget;
impl Serialize for DuplicateTarget {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("Response", 2)?;
        state.serialize_field("selected", &17)?;
        state.serialize_field("selected", &29)?;
        state.end()
    }
}

#[derive(Serialize)]
struct Response {
    ignored: Bad,
    selected: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    absent: Option<u32>,
}

#[test]
fn gamma_survivor_response_field_selection_fallback_and_failures() {
    let response = Response {
        ignored: Bad,
        selected: 37,
        absent: None,
    };
    let kind = ResponseBodyKind::Field {
        name: "selected",
        default: "0",
    };
    assert_eq!(encode_response(&response, kind).unwrap(), b"37");
    assert_eq!(encode_response(&Some(response), kind).unwrap(), b"37");
    assert_eq!(encode_response(&json!({"selected": [1,2]}), kind).unwrap(), b"[1,2]");
    assert_eq!(encode_response(&DuplicateTarget, kind).unwrap(), b"17");
    assert_eq!(
        encode_response(
            &Response {
                ignored: Bad,
                selected: 37,
                absent: None
            },
            ResponseBodyKind::Field {
                name: "absent",
                default: "null"
            }
        )
        .unwrap(),
        b"null"
    );
    let error = encode_response(
        &Response {
            ignored: Bad,
            selected: 37,
            absent: None,
        },
        ResponseBodyKind::Field {
            name: "ignored",
            default: "null",
        },
    )
    .unwrap_err();
    assert_eq!(error.code(), Code::Internal);
    assert!(error.to_string().contains("deliberate serialization failure"));
    assert!(std::error::Error::source(&error).is_some());
    let error = encode_response(&Bad, kind).unwrap_err();
    assert_eq!(error.code(), Code::Internal);
    assert!(error.to_string().contains("deliberate serialization failure"));
    for message in [json!(true), json!(12), json!("text"), json!([]), Value::Null, json!({"other": 1})] {
        let error = encode_response(&message, kind).unwrap_err();
        assert_eq!(error.code(), Code::Internal);
        assert!(error.to_string().contains("response_body field is absent"));
    }
    let frames = block_on(
        StreamingResponse::encode_response(
            stream::iter([Ok::<_, Status>(json!({"selected": 7})), Ok(json!({"selected": 9}))]),
            StreamEncoding::JsonArray,
            kind,
        )
        .into_frames()
        .collect::<Vec<_>>(),
    );
    assert_eq!(frames, [Ok(b"[7".to_vec()), Ok(b",9".to_vec()), Ok(b"]".to_vec())]);
    for encoding in [StreamEncoding::JsonArray, StreamEncoding::NdJson, StreamEncoding::Sse] {
        let frames = block_on(encode_frames(stream::iter([Ok::<_, Status>(Bad), Ok(Bad)]), encoding).collect::<Vec<_>>());
        assert_eq!(frames.len(), 1);
        let error = frames[0].as_ref().unwrap_err();
        assert_eq!(error.code(), Code::Internal);
        assert_eq!(
            error.message(),
            "failed to serialize a streamed message: deliberate serialization failure"
        );
    }
}

#[derive(Debug, Deserialize, PartialEq)]
struct Nested {
    value: String,
    number: u32,
}
#[derive(Debug, Deserialize, PartialEq)]
struct Request {
    nested: Nested,
    enabled: bool,
    values: Vec<i32>,
    optional: Option<String>,
}

#[test]
fn gamma_survivor_overlay_body_query_nested_repeated_and_names() {
    let body = br#"{"nested":{"value":"body","number":7},"enabled":false,"values":[99],"optional":"body"}"#;
    let query = [
        ("nested%2Evalue", "caf%C3%A9+%2B"),
        ("enabled", "true"),
        ("values", "-2"),
        ("values", "3"),
        ("optional", ""),
    ];
    let request: Request = decode_request(&query, body, RequestBodyKind::Whole).unwrap();
    assert_eq!(
        request,
        Request {
            nested: Nested {
                value: "café +".into(),
                number: 7
            },
            enabled: true,
            values: vec![-2, 3],
            optional: Some(String::new())
        }
    );
    let request: Request = decode_request(
        &[("nested.number", "42"), ("enabled", "false"), ("values", "4")],
        br#"{"value":"field","number":0}"#,
        RequestBodyKind::Field("nested"),
    )
    .unwrap();
    assert_eq!(
        request.nested,
        Nested {
            value: "field".into(),
            number: 42
        }
    );
    assert_eq!(request.values, [4]);
    let value: Value = decode_request(
        &[("foo_bar", "query"), ("nested.foo_bar", "new")],
        br#"{"fooBar":"body","foobar":"distinct","nested":{"fooBar":"old","other":1}}"#,
        RequestBodyKind::Whole,
    )
    .unwrap();
    assert!(value.get("fooBar").is_none());
    assert_eq!(value["foo_bar"], "query");
    assert_eq!(value["foobar"], "distinct");
    assert_eq!(value["nested"]["foo_bar"], "new");
    assert!(value["nested"].get("fooBar").is_none());
    assert_eq!(value["nested"]["other"], 1);
    let pairs = (0..12).map(|i| (format!("k{i}"), i.to_string())).collect::<Vec<_>>();
    let mut query = pairs.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect::<Vec<_>>();
    query.push(("k2", "20"));
    let map: BTreeMap<String, Vec<u32>> = decode_request(&query, b"", RequestBodyKind::None).unwrap();
    assert_eq!(map.len(), 12);
    assert_eq!(map["k2"], [2, 20]);
    assert_eq!(map["k11"], [11]);
}

#[test]
fn gamma_survivor_overlay_rejects_conflicts_duplicates_and_bad_values() {
    for (query, body, detail) in [
        (
            vec![("x", "1"), ("x.child", "2")],
            b"".as_slice(),
            "query field conflicts with a nested field",
        ),
        (
            vec![("x.child", "2"), ("x", "1")],
            b"".as_slice(),
            "query field conflicts with a nested field",
        ),
        (vec![("x", "1")], br#"{"foo_bar":1,"fooBar":2}"#.as_slice(), "duplicate field"),
        (vec![("x.child", "2")], br#"{"x":1}"#.as_slice(), "expected a JSON object"),
        (vec![("x", "1")], b"[]".as_slice(), "must be a JSON object"),
        (vec![("x", "1")], br#"{"x":0} garbage"#.as_slice(), "invalid request body JSON"),
        (vec![("%GG", "1")], b"".as_slice(), "query parameter name"),
        (vec![("x", "%FF")], b"".as_slice(), "query parameter value"),
    ] {
        let error = decode_request::<Value>(&query, body, RequestBodyKind::Whole).unwrap_err();
        assert_eq!(error.code(), Code::InvalidArgument);
        assert!(error.to_string().contains(detail), "{error}");
    }
    for value in ["+1", "01.2", ".5", "1e999", "inf", "-inf", "nan"] {
        let error = decode_request::<BTreeMap<String, f64>>(&[("x", value)], b"", RequestBodyKind::None).unwrap_err();
        assert_eq!(error.code(), Code::InvalidArgument, "{value}");
    }
    for value in [".5", "%2Boops", "-oops"] {
        let error = decode_request::<Value>(&[("x", value)], b"", RequestBodyKind::None).unwrap_err();
        assert!(
            error.to_string().contains("query number is not in canonical protobuf JSON form"),
            "{value}: {error}"
        );
    }
    let error = decode_request::<BTreeMap<String, String>>(&[("x", "1"), ("x", "2")], b"", RequestBodyKind::None).unwrap_err();
    assert!(error.to_string().contains("non-repeated query field"));
    let deep = format!("{}leaf", "a.".repeat(63));
    let value: Value = decode_request(&[(&deep, "done")], b"", RequestBodyKind::None).unwrap();
    assert_eq!(value.pointer(&format!("/{}leaf", "a/".repeat(63))).unwrap(), "done");
    let deeper = format!("a.{deep}");
    assert!(
        decode_request::<Value>(&[(&deeper, "done")], b"", RequestBodyKind::None)
            .unwrap_err()
            .to_string()
            .contains("nested too deeply")
    );
}

#[test]
fn gamma_survivor_raw_query_and_decode_budget_boundaries() {
    assert_eq!(split_query("/p?a=b?c"), ("/p", Some("a=b?c")));
    assert_eq!(split_query("/p?"), ("/p", Some("")));
    assert_eq!(
        parse_query("&&a=x=y&flag&=empty&&").as_slice(),
        [("a", "x=y"), ("flag", ""), ("", "empty")]
    );
    for (input, accepted) in [
        ("a=x&".repeat(128), true),
        ("a=x&".repeat(129), false),
        ("k".repeat(4096), true),
        ("k".repeat(4097), false),
        ("&".repeat(16384), true),
        ("&".repeat(16385), false),
    ] {
        assert_eq!(QueryPairs::parse_limited(&input).is_ok(), accepted);
    }
    let pairs = vec![("x", "1"); 128];
    let decoded: BTreeMap<String, Vec<u32>> = decode_request(&pairs, b"", RequestBodyKind::None).unwrap();
    assert_eq!(decoded["x"], vec![1; 128]);
    let too_many = vec![("x", "1"); 129];
    assert!(
        decode_request::<Value>(&too_many, b"", RequestBodyKind::None)
            .unwrap_err()
            .to_string()
            .contains("parameter count")
    );
    let name = "k".repeat(4096);
    assert_eq!(
        decode_request::<Value>(&[(&name, "x")], b"", RequestBodyKind::None).unwrap(),
        Value::Object(serde_json::Map::from_iter([(name.clone(), json!("x"))]))
    );
    assert!(
        decode_request::<Value>(&[(&format!("{name}k"), "x")], b"", RequestBodyKind::None)
            .unwrap_err()
            .to_string()
            .contains("field-name size")
    );
    let value = "x".repeat(16383);
    let decoded: BTreeMap<String, String> = decode_request(&[("k", &value)], b"", RequestBodyKind::None).unwrap();
    assert_eq!(decoded["k"], value);
    assert!(
        decode_request::<Value>(&[("k", &format!("{value}x"))], b"", RequestBodyKind::None)
            .unwrap_err()
            .to_string()
            .contains("query size")
    );
}

#[test]
fn gamma_survivor_path_scalars_reserved_expansion_enum_and_bytes() {
    fn from_name(name: &str) -> Option<i32> {
        (name == "ACTIVE").then_some(7)
    }
    assert_eq!(parse_path_field::<String>("a%2Fb+c%25%C3%A9").unwrap(), "a/b+c%é");
    assert_eq!(parse_reserved_path_field::<String>("a%2fb%3F%23%25%41").unwrap(), "a%2fb?#%A");
    assert_eq!(parse_path_field::<Option<u32>>("%34%32").unwrap(), Some(42));
    for encoded in ["+/8=", "-_8=", "+/8", "-_8"] {
        assert_eq!(parse_path_field::<Vec<u8>>(encoded).unwrap(), [251, 255]);
        assert_eq!(parse_path_field::<bytes::Bytes>(encoded).unwrap().as_ref(), [251, 255]);
    }
    assert_eq!(parse_path_enum_value("%41CTIVE", from_name).unwrap(), 7);
    assert_eq!(parse_reserved_path_enum_value("%41CTIVE", from_name).unwrap(), 7);
    assert_eq!(parse_path_enum_value("-17", from_name).unwrap(), -17);
    assert_eq!(parse_reserved_path_enum_value("23", from_name).unwrap(), 23);
    for raw in ["%", "%0", "%G0", "%0G", "%FF"] {
        assert_eq!(parse_path_field::<String>(raw).unwrap_err().code(), Code::InvalidArgument);
        assert_eq!(parse_path_enum_value(raw, from_name).unwrap_err().code(), Code::InvalidArgument);
        assert_eq!(parse_reserved_path_field::<String>(raw).unwrap_err().code(), Code::InvalidArgument);
        assert_eq!(
            parse_reserved_path_enum_value(raw, from_name).unwrap_err().code(),
            Code::InvalidArgument
        );
    }
    assert!(
        parse_path_enum_value("UNKNOWN", from_name)
            .unwrap_err()
            .to_string()
            .contains("UNKNOWN")
    );
    assert!(
        parse_reserved_path_enum_value("UNKNOWN", from_name)
            .unwrap_err()
            .to_string()
            .contains("UNKNOWN")
    );
    assert_eq!(parse_path_field::<Vec<u8>>("%%bad").unwrap_err().code(), Code::InvalidArgument);
    assert!(parse_path_field::<bool>("true").unwrap());
    assert_eq!(parse_path_field::<bool>("True").unwrap_err().code(), Code::InvalidArgument);
    assert_eq!(parse_path_field::<i64>("-9223372036854775808").unwrap(), i64::MIN);
    assert_eq!(parse_path_field::<u64>("18446744073709551615").unwrap(), u64::MAX);
    assert!(parse_path_field::<f64>("NaN").unwrap().is_nan());
    assert_eq!(parse_path_field::<f32>("-Infinity").unwrap().to_bits(), f32::NEG_INFINITY.to_bits());
    for raw in ["inf", "+1", ".5", "01", "1e999"] {
        assert!(parse_path_field::<f64>(raw).is_err(), "{raw}");
    }
}

#[test]
fn gamma_survivor_context_and_wire_headers_preserve_repeats_strip_framing() {
    let mut request = HeaderMap::new();
    request.insert(header::AUTHORIZATION, HeaderValue::from_static("private-request"));
    let mut context = Context::new(request);
    context.insert_response_header(header::SET_COOKIE, HeaderValue::from_static("private-cookie"));
    let debug = format!("{context:?}");
    assert!(debug.contains("authorization"));
    assert!(debug.contains("set-cookie"));
    assert!(!debug.contains("private-request"));
    assert!(!debug.contains("private-cookie"));
    let mut extra = HeaderMap::new();
    for value in ["a=1", "b=2"] {
        extra.append(header::SET_COOKIE, HeaderValue::from_static(value));
    }
    extra.append(header::CONNECTION, HeaderValue::from_static("x-hop, close"));
    extra.append(header::CONNECTION, HeaderValue::from_static(" x-other "));
    for name in [
        "x-hop",
        "x-other",
        "content-length",
        "transfer-encoding",
        "te",
        "trailer",
        "upgrade",
        "proxy-authenticate",
        "proxy-authorization",
        "keep-alive",
        "proxy-connection",
    ] {
        extra.insert(name, HeaderValue::from_static("discard"));
    }
    extra.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain"));
    extra.insert(header::ETAG, HeaderValue::from_static("retain"));
    context.merge_response_headers(extra.clone());
    assert_eq!(context.response_headers().get_all(header::SET_COOKIE).iter().count(), 3);
    let mut unary = HttpResponse::ok_json(b"{}".to_vec());
    unary.merge_headers(context.into_response_headers());
    let http = unary.into_http();
    assert_eq!(http.headers()[header::CONTENT_TYPE], "application/json");
    assert_eq!(http.headers()[header::ETAG], "retain");
    assert_eq!(http.headers().get_all(header::SET_COOKIE).iter().count(), 3);
    for name in [
        "connection",
        "x-hop",
        "x-other",
        "content-length",
        "transfer-encoding",
        "te",
        "trailer",
        "upgrade",
        "proxy-authenticate",
        "proxy-authorization",
        "keep-alive",
        "proxy-connection",
    ] {
        assert!(!http.headers().contains_key(name), "{name}");
    }
    let mut streaming = StreamingResponse::encode(stream::empty::<Result<u32, Status>>(), StreamEncoding::Sse);
    streaming.merge_headers(extra);
    assert_eq!(streaming.headers().get_all(header::SET_COOKIE).iter().count(), 2);
    assert!(!streaming.headers().contains_key("x-hop"));
    let (content_type, headers, _) = streaming.into_parts();
    assert_eq!(content_type, "text/event-stream");
    assert_eq!(headers[header::ETAG], "retain");
    let error = HttpResponse::from_status(&Status::new(Code::PermissionDenied, "denied").with_detail(json!({"reason":"policy"})));
    assert_eq!(error.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        serde_json::from_slice::<Value>(error.body()).unwrap(),
        json!({"code":7,"message":"denied","details":[{"reason":"policy"}]})
    );
}

#[cfg(feature = "serving")]
#[test]
fn gamma_survivor_serving_incremental_body_and_stream_error() {
    use bytes::Bytes;
    use http_body::Frame;
    use http_body_util::{BodyExt, StreamBody};
    use rest_over_grpc::serving::serve_http_fn_with_max_body_bytes;

    for (max, expected) in [(5, StatusCode::OK), (4, StatusCode::PAYLOAD_TOO_LARGE)] {
        let frames = stream::iter([
            Ok::<_, std::io::Error>(Frame::data(Bytes::from_static(b"ab"))),
            Ok(Frame::trailers(HeaderMap::new())),
            Ok(Frame::data(Bytes::from_static(b"cde"))),
        ]);
        let request = http::Request::builder()
            .method("PATCH")
            .uri("/p?x=y")
            .header("x-request", "marker")
            .body(StreamBody::new(frames))
            .unwrap();
        let response = block_on(serve_http_fn_with_max_body_bytes(
            request,
            max,
            |method, uri, headers, body| async move {
                assert_eq!(method, "PATCH");
                assert_eq!(uri.to_string(), "/p?x=y");
                assert_eq!(headers["x-request"], "marker");
                assert_eq!(body, b"abcde".as_slice());
                HttpResponse::ok_json(body.to_vec())
            },
        ));
        assert_eq!(response.status(), expected);
        let body = block_on(response.into_body().collect()).unwrap().to_bytes();
        if max == 5 {
            assert_eq!(body, b"abcde".as_slice());
        }
    }
    let response = StreamingResponse::encode(
        stream::iter([Ok(1), Err(Status::new(Code::Aborted, "truncated")), Ok(99)]),
        StreamEncoding::JsonArray,
    );
    let request = http::Request::new(http_body_util::Empty::<Bytes>::new());
    let response = block_on(rest_over_grpc::serving::serve_http_fn(
        request,
        |_, _, _, _| async move { response },
    ));
    let error = block_on(response.into_body().collect()).unwrap_err();
    assert_eq!(error.to_string(), "server-streaming response failed: truncated (ABORTED)");
}

#[test]
fn gamma_survivor_query_scalar_repeats_eof_and_precise_error_details() {
    let pairs = [("x", "1"), ("x", "2")];
    macro_rules! repeated {
        ($($ty:ty),+ $(,)?) => {$(
            let error = decode_request::<BTreeMap<String, $ty>>(&pairs, b"", RequestBodyKind::None).unwrap_err();
            assert_eq!(error.code(), Code::InvalidArgument);
            assert!(error.to_string().contains("a non-repeated query field cannot receive multiple values"));
        )+};
    }
    repeated!(bool, f32, f64, String, Value, Option<String>, u32);
    let error = decode_request::<BTreeMap<String, f32>>(&[("x", "1e39")], b"", RequestBodyKind::None).unwrap_err();
    assert!(error.to_string().contains("float is out of range"), "{error}");
    let decoded: BTreeMap<String, f32> = decode_request(
        &[
            ("finite", "1.25"),
            ("not_a_number", "NaN"),
            ("positive", "Infinity"),
            ("negative", "-Infinity"),
        ],
        b"",
        RequestBodyKind::None,
    )
    .unwrap();
    assert_eq!(decoded["finite"].to_bits(), 1.25_f32.to_bits());
    assert!(decoded["not_a_number"].is_nan());
    assert_eq!(decoded["positive"].to_bits(), f32::INFINITY.to_bits());
    assert_eq!(decoded["negative"].to_bits(), f32::NEG_INFINITY.to_bits());
    let decoded: BTreeMap<String, String> = decode_request(&[("a+b", "a+b"), ("plain", "retained")], b"", RequestBodyKind::None).unwrap();
    assert_eq!(
        decoded,
        BTreeMap::from([("a b".into(), "a b".into()), ("plain".into(), "retained".into())])
    );
    for (kind, prefix) in [
        (RequestBodyKind::Whole, "invalid request body JSON:"),
        (RequestBodyKind::Field("x"), "request did not match the expected message:"),
    ] {
        let error = decode_request::<Value>(&[], b"{", kind).unwrap_err();
        assert!(error.to_string().starts_with(prefix), "{error}");
        assert_eq!(error.code(), Code::InvalidArgument);
    }
    let error = decode_request::<Value>(&[("x", "1")], br#"{"nested":{"leaf":"#, RequestBodyKind::Whole).unwrap_err();
    assert!(error.to_string().contains("invalid request body JSON"), "{error}");
    for query in ["&".repeat(16385), "a=x&".repeat(129), "k".repeat(4097)] {
        let error = QueryPairs::parse_limited(&query).unwrap_err();
        let expected = if query.len() > 16384 {
            "query exceeds the request query size limit"
        } else {
            "query exceeds the parameter count or field-name size limit"
        };
        assert!(error.to_string().contains(expected), "{error}");
    }
    let missing = HttpResponse::not_found();
    assert_eq!(
        serde_json::from_slice::<Value>(missing.body()).unwrap()["message"],
        "no route matches the request"
    );
    let mut headers = HeaderMap::new();
    headers.insert(header::AUTHORIZATION, HeaderValue::from_static("secret"));
    let context = Context::new(headers);
    assert_eq!(
        format!("{context:?}"),
        "Context { request_header_names: [\"authorization\"], response_header_names: [] }"
    );
    let invalid = parse_path_field::<String>("%FF").unwrap_err();
    assert!(invalid.to_string().contains("path variable"), "{invalid}");
    let invalid = parse_reserved_path_field::<String>("%FF").unwrap_err();
    assert!(invalid.to_string().contains("path variable"), "{invalid}");
    let invalid = parse_path_enum_value::<i32>("%FF", |_| None).unwrap_err();
    assert!(invalid.to_string().contains("path variable"), "{invalid}");
    let invalid = parse_reserved_path_enum_value::<i32>("%FF", |_| None).unwrap_err();
    assert!(invalid.to_string().contains("path variable"), "{invalid}");
}

#[test]
fn gamma_survivor_decode_fast_path_source_and_map_access_errors() {
    #[derive(Debug)]
    struct RejectDecode;
    impl<'de> Deserialize<'de> for RejectDecode {
        fn deserialize<D: serde::Deserializer<'de>>(_: D) -> Result<Self, D::Error> {
            Err(serde::de::Error::custom("visitor sentinel"))
        }
    }
    #[derive(Debug)]
    struct ValueBeforeKey;
    impl<'de> Deserialize<'de> for ValueBeforeKey {
        fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            struct Visitor;
            impl<'de> serde::de::Visitor<'de> for Visitor {
                type Value = ValueBeforeKey;
                fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    formatter.write_str("a map")
                }
                fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                    let _: Value = map.next_value()?;
                    Ok(ValueBeforeKey)
                }
            }
            deserializer.deserialize_map(Visitor)
        }
    }
    let error = decode_request::<RejectDecode>(&[], b"", RequestBodyKind::None).unwrap_err();
    assert!(std::error::Error::source(&error).unwrap().is::<serde::de::value::Error>());
    assert!(error.to_string().contains("visitor sentinel"));
    assert_eq!(decode_request::<Value>(&[], b"", RequestBodyKind::Whole).unwrap(), json!({}));

    let error = decode_request::<ValueBeforeKey>(&[("x", "1")], b"", RequestBodyKind::None).unwrap_err();
    assert!(error.to_string().contains("value requested before key"), "{error}");
}

#[cfg(feature = "serving")]
#[test]
fn gamma_survivor_serving_default_body_limit_is_inclusive() {
    use bytes::Bytes;
    use http_body_util::Full;
    use rest_over_grpc::serving::{DEFAULT_MAX_BODY_BYTES, serve_http_fn};
    for (length, expected) in [
        (DEFAULT_MAX_BODY_BYTES, StatusCode::OK),
        (DEFAULT_MAX_BODY_BYTES + 1, StatusCode::PAYLOAD_TOO_LARGE),
    ] {
        let request = http::Request::new(Full::new(Bytes::from(vec![b'x'; length])));
        let response = block_on(serve_http_fn(request, |_, _, _, body| async move {
            assert_eq!(body.len(), DEFAULT_MAX_BODY_BYTES);
            HttpResponse::ok_json(Vec::new())
        }));
        assert_eq!(response.status(), expected);
    }
}
