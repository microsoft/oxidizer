// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Build API regression oracles using descriptors assembled entirely in memory.

#![cfg(feature = "build")]

use http_path_template::{Grammar, PathTemplate};
use rest_over_grpc::build::{Binding, DescriptorOptions, Generator, HttpRule, RequestBody, ResponseBody, ServiceDefinition};
use routerama::HttpMethod;

#[cfg(test)]
fn varint(mut value: u64) -> Vec<u8> {
    let mut bytes = Vec::new();
    while value >= 128 {
        bytes.push(u8::try_from(value & 127).unwrap() | 128);
        value >>= 7;
    }
    bytes.push(u8::try_from(value).unwrap());
    bytes
}

fn number(field: u64, value: u64) -> Vec<u8> {
    [varint(field << 3), varint(value)].concat()
}

#[cfg(test)]
fn bytes(field: u64, value: &[u8]) -> Vec<u8> {
    [varint((field << 3) | 2), varint(value.len().try_into().unwrap()), value.to_vec()].concat()
}

fn text(field: u64, value: &str) -> Vec<u8> {
    bytes(field, value.as_bytes())
}

fn field(name: &str, index: u64, kind: u64, type_name: &str, repeated: bool) -> Vec<u8> {
    let mut result = [
        text(1, name),
        number(3, index),
        number(4, if repeated { 3 } else { 1 }),
        number(5, kind),
    ]
    .concat();
    if !type_name.is_empty() {
        result.extend(text(6, type_name));
    }
    result
}

fn message(name: &str, fields: Vec<Vec<u8>>) -> Vec<u8> {
    let mut result = text(1, name);
    for field in fields {
        result.extend(bytes(2, &field));
    }
    result
}

fn annotation(method_field: u64, path: &str, body: &str, response: &str) -> Vec<u8> {
    [text(method_field, path), text(7, body), text(12, response)].concat()
}

fn descriptor(annotation: &[u8], client_streaming: bool, server_streaming: bool) -> Vec<u8> {
    descriptor_with_comments(annotation, client_streaming, server_streaming, true)
}

fn descriptor_with_comments(annotation: &[u8], client_streaming: bool, server_streaming: bool, comments: bool) -> Vec<u8> {
    descriptor_with_doc_lines(
        annotation,
        client_streaming,
        server_streaming,
        comments.then_some(" Service documentation. \n"),
        comments.then_some(" Method documentation. \n"),
    )
}

fn descriptor_with_doc_lines(
    annotation: &[u8],
    client_streaming: bool,
    server_streaming: bool,
    service_doc: Option<&str>,
    method_doc: Option<&str>,
) -> Vec<u8> {
    descriptor_with_extension_shape(annotation, client_streaming, server_streaming, service_doc, method_doc, true)
}

fn descriptor_with_extension_shape(
    annotation: &[u8],
    client_streaming: bool,
    server_streaming: bool,
    service_doc: Option<&str>,
    method_doc: Option<&str>,
    message_extension: bool,
) -> Vec<u8> {
    let (descriptor_file, http_file) = http_descriptors(message_extension);
    api_descriptor(
        annotation,
        client_streaming,
        server_streaming,
        service_doc,
        method_doc,
        &descriptor_file,
        &http_file,
    )
}

#[cfg(test)]
fn http_descriptors(message_extension: bool) -> (Vec<u8>, Vec<u8>) {
    let method_options = [text(1, "MethodOptions"), bytes(5, &[number(1, 1), number(2, 536_870_912)].concat())].concat();
    let descriptor_file = [
        text(1, "google/protobuf/descriptor.proto"),
        text(2, "google.protobuf"),
        bytes(4, &method_options),
        text(12, "proto2"),
    ]
    .concat();
    let http_rule = message(
        "HttpRule",
        vec![
            field("get", 2, 9, "", false),
            field("put", 3, 9, "", false),
            field("post", 4, 9, "", false),
            field("delete", 5, 9, "", false),
            field("patch", 6, 9, "", false),
            field("body", 7, 9, "", false),
            field("custom", 8, 11, ".google.api.CustomHttpPattern", false),
            field("additional_bindings", 11, 11, ".google.api.HttpRule", true),
            field("response_body", 12, 9, "", false),
        ],
    );
    let custom = message(
        "CustomHttpPattern",
        vec![field("kind", 1, 9, "", false), field("path", 2, 9, "", false)],
    );
    let extension = [
        field(
            "http",
            72_295_728,
            if message_extension { 11 } else { 9 },
            if message_extension { ".google.api.HttpRule" } else { "" },
            false,
        ),
        text(2, ".google.protobuf.MethodOptions"),
    ]
    .concat();
    let http_file = [
        text(1, "http.proto"),
        text(2, "google.api"),
        text(3, "google/protobuf/descriptor.proto"),
        bytes(4, &http_rule),
        bytes(4, &custom),
        bytes(7, &extension),
        bytes(6, &text(1, "UnannotatedService")),
        text(12, "proto2"),
    ]
    .concat();
    (descriptor_file, http_file)
}

#[cfg(test)]
fn api_descriptor(
    annotation: &[u8],
    client_streaming: bool,
    server_streaming: bool,
    service_doc: Option<&str>,
    method_doc: Option<&str>,
    descriptor_file: &[u8],
    http_file: &[u8],
) -> Vec<u8> {
    let nested = message(
        "Nested",
        vec![
            field("leaf", 1, 9, "", false),
            field("count", 2, 5, "", false),
            field("self_ref", 3, 11, ".demo.api.Nested", false),
        ],
    );
    let mut input = message(
        "Input",
        vec![
            field("book_id", 1, 9, "", false),
            field("nested", 2, 11, ".demo.api.Nested", false),
            field("values", 3, 5, "", true),
            field("state", 4, 14, ".demo.api.State", false),
        ],
    );
    let map_entry = [
        message("LabelsEntry", vec![field("key", 1, 9, "", false), field("value", 2, 9, "", false)]),
        bytes(7, &number(7, 1)),
    ]
    .concat();
    input.extend(bytes(3, &map_entry));
    input.extend(bytes(2, &field("labels", 5, 11, ".demo.api.Input.LabelsEntry", true)));
    let output = message(
        "Output",
        vec![
            field("result_code", 1, 3, "", false),
            field("enabled", 2, 8, "", false),
            field("label", 3, 9, "", false),
            field("values", 4, 5, "", true),
            field("nested", 5, 11, ".demo.api.Nested", false),
            field("state", 6, 14, ".demo.api.State", false),
        ],
    );
    let enumeration = [
        text(1, "State"),
        bytes(2, &[text(1, "UNSPECIFIED"), number(2, 0)].concat()),
        bytes(2, &[text(1, "ACTIVE"), number(2, 7)].concat()),
    ]
    .concat();
    let options = bytes(72_295_728, annotation);
    let method = [
        text(1, "GetBook"),
        text(2, ".demo.api.Input"),
        text(3, ".demo.api.Output"),
        bytes(4, &options),
        number(5, u64::from(client_streaming)),
        number(6, u64::from(server_streaming)),
    ]
    .concat();
    let service = [text(1, "BookService"), bytes(2, &method)].concat();
    let source_info = [
        service_doc.map_or_else(Vec::new, |doc| bytes(1, &[number(1, 6), number(1, 0), text(3, doc)].concat())),
        method_doc.map_or_else(Vec::new, |doc| {
            bytes(1, &[number(1, 6), number(1, 0), number(1, 2), number(1, 0), text(3, doc)].concat())
        }),
    ]
    .concat();
    let api_file = [
        text(1, "api.proto"),
        text(2, "demo.api"),
        text(3, "http.proto"),
        bytes(4, &input),
        bytes(4, &output),
        bytes(4, &nested),
        bytes(5, &enumeration),
        bytes(6, &service),
        if source_info.is_empty() {
            Vec::new()
        } else {
            bytes(9, &source_info)
        },
        text(12, "proto3"),
    ]
    .concat();
    [bytes(1, descriptor_file), bytes(1, http_file), bytes(1, &api_file)].concat()
}

#[cfg(test)]
fn generated(descriptor: &[u8], options: &DescriptorOptions) -> (String, String) {
    let definitions = ServiceDefinition::from_fds(descriptor, options).unwrap();
    assert_eq!(definitions.len(), 1);
    assert_eq!(definitions[0].trait_name(), "BookService");
    assert_eq!(definitions[0].module_name(), "demo.api");
    let (transcoder, outputs) = Generator::new().add_all(definitions).generate().unwrap();
    assert_eq!(outputs.len(), 1);
    assert_eq!(outputs[0].module_name(), "demo.api");
    assert_eq!(outputs[0].trait_name(), "BookService");
    assert!(outputs[0].tonic_bridge().is_some());
    (transcoder.to_string(), outputs[0].service_trait().to_string())
}

#[test]
fn gamma_survivor_descriptor_annotations_types_comments_and_prefixes() {
    let descriptor = descriptor(&annotation(2, "/v/{book_id}", "nested", "result_code"), false, false);
    let (transcoder, service) = generated(&descriptor, &DescriptorOptions::new().package(".demo").package(".other"));
    for token in [
        "book_id",
        "RequestBodyKind :: Field (\"nested\")",
        "name : \"resultCode\"",
        "default : \"\\\"0\\\"\"",
    ] {
        assert!(transcoder.contains(token), "{token}: {transcoder}");
    }
    assert!(service.contains("Service documentation."));
    assert!(service.contains("Method documentation."));
    assert!(service.contains("get_book"));
    for prefix in [".dem", ".demo.ap", ".demo.api.Book", ".other"] {
        assert!(
            ServiceDefinition::from_fds(&descriptor, &DescriptorOptions::new().package(prefix))
                .unwrap()
                .is_empty(),
            "{prefix}"
        );
    }
    assert_eq!(
        ServiceDefinition::from_fds(&descriptor, &DescriptorOptions::new().package("demo.api.BookService"))
            .unwrap()
            .len(),
        1
    );
    let (_, service) = generated(
        &descriptor,
        &DescriptorOptions::new()
            .extern_path(".demo", "broad")
            .extern_path(".demo.api.Input", "crate::ExactInput"),
    );
    assert!(service.contains("crate :: ExactInput"));
    assert!(service.contains(":: broad :: api :: Output"));
}

#[test]
fn gamma_survivor_descriptor_errors_and_body_defaults() {
    for (rule, client, detail) in [
        (annotation(2, "/v/{missing}", "", ""), false, "missing"),
        (annotation(2, "/v/{book_id.leaf}", "", ""), false, "non-message"),
        (annotation(2, "/v/{nested}", "", ""), false, "primitive or enum"),
        (annotation(2, "/v/{values}", "", ""), false, "repeated or map"),
        (annotation(2, "/v/{labels}", "", ""), false, "repeated or map"),
        (annotation(2, "/v/{book_id}", "missing", ""), false, "body"),
        (annotation(2, "/v/{book_id}", "", "missing"), false, "response_body"),
        (annotation(2, "/v/{", "", ""), false, "path template"),
        (annotation(2, "/v/{book_id}", "", ""), true, "streaming"),
        (Vec::new(), false, "pattern"),
        (bytes(8, &[text(1, ""), text(2, "/custom")].concat()), false, "kind"),
        (bytes(8, &[text(1, "M-SEARCH"), text(2, "")].concat()), false, "path"),
        (
            bytes(8, &[text(1, "bad method"), text(2, "/custom")].concat()),
            false,
            "HTTP method",
        ),
        (
            [
                annotation(2, "/v/{book_id}", "", ""),
                bytes(
                    11,
                    &[
                        annotation(2, "/alias/{book_id}", "", ""),
                        bytes(11, &annotation(2, "/nested/{book_id}", "", "")),
                    ]
                    .concat(),
                ),
            ]
            .concat(),
            false,
            "nested",
        ),
    ] {
        let error = ServiceDefinition::from_fds(descriptor(&rule, client, false), &DescriptorOptions::new()).unwrap_err();
        assert!(error.to_string().contains(detail), "{detail}: {error}");
    }
    let error = ServiceDefinition::from_fds(b"\xff", &DescriptorOptions::new()).unwrap_err();
    assert!(error.to_string().contains("descriptor"));
    let non_message = descriptor_with_extension_shape(&annotation(2, "/v/{book_id}", "", ""), false, false, None, None, false);
    let error = ServiceDefinition::from_fds(&non_message, &DescriptorOptions::new()).unwrap_err();
    assert!(error.to_string().contains("http annotation is not a message"), "{error}");
    for (field, default) in [
        ("result_code", "\\\"0\\\""),
        ("enabled", "false"),
        ("label", "\\\"\\\""),
        ("values", "[]"),
        ("nested", "null"),
        ("state", "\\\"UNSPECIFIED\\\""),
    ] {
        let (transcoder, _) = generated(
            &descriptor(&annotation(2, "/v/{book_id}", "", field), false, false),
            &DescriptorOptions::new(),
        );
        assert!(transcoder.contains(&format!("default : \"{default}\"")), "{field}: {transcoder}");
    }
}

#[test]
fn gamma_survivor_descriptor_methods_additional_bindings_and_streaming() {
    for (index, method) in [(2, "GET"), (3, "PUT"), (4, "POST"), (5, "DELETE"), (6, "PATCH")] {
        let rule = [
            annotation(index, "/v/{nested.leaf}", "*", ""),
            bytes(11, &annotation(2, "/alias/{state}", "", "enabled")),
        ]
        .concat();
        let (transcoder, service) = generated(&descriptor(&rule, false, true), &DescriptorOptions::new());
        assert!(transcoder.contains(&format!("\"{method}\"")), "{transcoder}");
        for token in [
            "alias",
            "parse_path_enum_value",
            "from_str_name",
            "nested",
            "get_or_insert_with",
            "StreamEncoding",
            "RequestBodyKind :: Whole",
        ] {
            assert!(transcoder.contains(token), "{token}: {transcoder}");
        }
        assert!(service.contains("ResponseStream"), "{service}");
    }
    let rule = bytes(8, &[text(1, "M-SEARCH"), text(2, "/custom/{book_id}")].concat());
    assert!(
        generated(&descriptor(&rule, false, false), &DescriptorOptions::new())
            .0
            .contains("\"M-SEARCH\"")
    );
}

#[test]
fn gamma_nov4_descriptor_pattern_priority_is_not_reversed_for_non_oneof_extensions() {
    let rule = [
        annotation(2, "/first", "", ""),
        annotation(3, "/second", "", ""),
        annotation(6, "/last", "", ""),
    ]
    .concat();
    let (transcoder, _) = generated(&descriptor(&rule, false, false), &DescriptorOptions::new());
    assert!(transcoder.contains("\"GET\""), "{transcoder}");
    assert!(transcoder.contains("b\"first\""), "{transcoder}");
    assert!(!transcoder.contains("\"PUT\""), "{transcoder}");
    assert!(!transcoder.contains("\"PATCH\""), "{transcoder}");
    assert!(!transcoder.contains("b\"second\""), "{transcoder}");
    assert!(!transcoder.contains("b\"last\""), "{transcoder}");
}

#[cfg(feature = "build-openapi")]
#[test]
fn gamma_nov4_openapi_well_known_queries_keep_scalar_and_array_shapes_without_message_lists() {
    use rest_over_grpc::build::OpenApiInfo;

    let (descriptor_file, http_file) = http_descriptors(true);
    let timestamp_file = [
        text(1, "timestamp.proto"),
        text(2, "google.protobuf"),
        bytes(
            4,
            &message(
                "Timestamp",
                vec![field("seconds", 1, 3, "", false), field("nanos", 2, 5, "", false)],
            ),
        ),
        text(12, "proto3"),
    ]
    .concat();
    let input = message(
        "Input",
        vec![
            field("instant", 1, 11, ".google.protobuf.Timestamp", false),
            field("instants", 2, 11, ".google.protobuf.Timestamp", true),
            field("messages", 3, 11, ".demo.api.Output", true),
        ],
    );
    let method = [
        text(1, "GetBook"),
        text(2, ".demo.api.Input"),
        text(3, ".demo.api.Output"),
        bytes(4, &bytes(72_295_728, &annotation(2, "/probe", "", ""))),
    ]
    .concat();
    let api = [
        text(1, "api.proto"),
        text(2, "demo.api"),
        text(3, "http.proto"),
        text(3, "timestamp.proto"),
        bytes(4, &input),
        bytes(4, &message("Output", Vec::new())),
        bytes(6, &[text(1, "BookService"), bytes(2, &method)].concat()),
        text(12, "proto3"),
    ]
    .concat();
    let descriptors = [
        bytes(1, &descriptor_file),
        bytes(1, &http_file),
        bytes(1, &timestamp_file),
        bytes(1, &api),
    ]
    .concat();
    let definitions = ServiceDefinition::from_fds(descriptors, &DescriptorOptions::new()).unwrap();
    let (_, output) = Generator::builder()
        .emit_openapi_spec(Some(OpenApiInfo::new("In-memory timestamp query", "1")))
        .build()
        .add_all(definitions)
        .generate()
        .unwrap();
    let spec: serde_json::Value = serde_json::from_str(output[0].openapi_spec().unwrap()).unwrap();
    let parameters = spec["paths"]["/probe"]["get"]["parameters"].as_array().unwrap();
    assert_eq!(parameters.len(), 2);
    assert_eq!(parameters[0]["name"], "instant");
    assert_eq!(parameters[0]["in"], "query");
    assert_eq!(parameters[0]["schema"], serde_json::json!({"type":"string","format":"date-time"}));
    assert_eq!(parameters[1]["name"], "instants");
    assert_eq!(
        parameters[1]["schema"],
        serde_json::json!({"type":"array","items":{"type":"string","format":"date-time"}})
    );
}

#[test]
fn gamma_survivor_manual_generation_options_and_alias_capture_groups() {
    let template = |path: &'static str| PathTemplate::parse(path, Grammar::default()).unwrap();
    let binding = Binding::new(HttpMethod::PUT, template("/alias/{other}"))
        .request_body(RequestBody::Whole)
        .response_body(ResponseBody::Field("result".into()));
    assert_eq!(binding.method(), &HttpMethod::PUT);
    assert_eq!(binding.template().to_string(), "/alias/{other}");
    let rule = HttpRule::new("GetBook", HttpMethod::GET, template("/books/{book}"))
        .response_body(ResponseBody::Field("primary".into()))
        .add_binding(binding)
        .add_binding(Binding::new(HttpMethod::GET, template("/unset/{book}")));
    assert_eq!(rule.name(), "GetBook");
    let mut service = ServiceDefinition::new("LibraryAPI", Some("docs".into()));
    service
        .set_module_name("catalog")
        .add_method(rule, "crate::Input", "crate::Output", Some("method docs".into()));
    let (transcoder, outputs) = Generator::builder()
        .emit_tonic_bridge(false)
        .build()
        .add(service)
        .generate()
        .unwrap();
    assert_eq!(outputs[0].module_name(), "catalog");
    assert_eq!(outputs[0].trait_name(), "LibraryAPI");
    assert!(outputs[0].tonic_bridge().is_none());
    let transcoder = transcoder.to_string();
    for token in ["book", "other", "alias", "RequestBodyKind :: Whole", "name : \"result\""] {
        assert!(transcoder.contains(token), "{token}: {transcoder}");
    }
    assert!(
        transcoder.contains("unset") && transcoder.contains("name : \"primary\""),
        "{transcoder}"
    );
    assert!(transcoder.contains("ResponseBodyKind :: Whole"), "{transcoder}");
    assert!(transcoder.contains("name : \"result\" , default : \"null\""), "{transcoder}");
    syn::parse2::<syn::File>(outputs[0].service_trait().clone()).unwrap();
}

#[cfg(feature = "build-openapi")]
#[test]
fn gamma_survivor_openapi_paths_filters_recursive_schema_and_streaming() {
    use rest_over_grpc::build::OpenApiInfo;
    let rule = [
        annotation(2, "/v/{nested.leaf}", "bookId", "result_code"),
        bytes(11, &annotation(4, "/alias/{book_id}", "*", "")),
    ]
    .concat();
    let services = ServiceDefinition::from_fds(descriptor(&rule, false, true), &DescriptorOptions::new()).unwrap();
    let (_, outputs) = Generator::builder()
        .emit_tonic_bridge(false)
        .emit_openapi_spec(Some(OpenApiInfo::new("Gamma API", "v7")))
        .build()
        .add_all(services)
        .generate()
        .unwrap();
    let spec: serde_json::Value = serde_json::from_str(outputs[0].openapi_spec().unwrap()).unwrap();
    assert_eq!(spec["openapi"], "3.1.0");
    assert_eq!(spec["info"]["title"], "Gamma API");
    assert_eq!(spec["info"]["version"], "v7");
    let operation = &spec["paths"]["/v/{nested.leaf}"]["get"];
    let parameters = operation["parameters"].as_array().unwrap();
    assert_eq!(
        parameters.iter().map(|p| p["name"].as_str().unwrap()).collect::<Vec<_>>(),
        ["nested.leaf", "nested.count", "values", "state"]
    );
    assert_eq!(parameters[0]["in"], "path");
    assert_eq!(parameters[0]["required"], true);
    assert_eq!(operation["requestBody"]["content"]["application/json"]["schema"]["type"], "string");
    assert_eq!(
        operation["responses"]["200"]["content"]["application/json"]["schema"]["type"],
        "array"
    );
    assert_eq!(
        operation["responses"]["200"]["content"]["application/json"]["schema"]["items"]["format"],
        "int64"
    );
    let properties = &spec["components"]["schemas"]["demo.api.Nested"]["properties"];
    assert_eq!(properties["selfRef"]["$ref"], "#/components/schemas/demo.api.Nested");
    assert_eq!(
        spec["components"]["schemas"]["demo.api.State"]["enum"],
        serde_json::json!(["UNSPECIFIED", "ACTIVE"])
    );
    assert_eq!(spec["paths"]["/alias/{book_id}"]["post"]["parameters"].as_array().unwrap().len(), 1);
    assert!(spec["components"]["schemas"].get("google.rpc.Status").is_some());
    let duplicate = [
        annotation(2, "/same/{book_id}", "", ""),
        bytes(11, &annotation(2, "/same/{book_id}", "", "")),
    ]
    .concat();
    let services = ServiceDefinition::from_fds(descriptor(&duplicate, false, false), &DescriptorOptions::new()).unwrap();
    let error = Generator::builder()
        .emit_openapi_spec(Some(OpenApiInfo::new("Duplicate", "1")))
        .build()
        .add_all(services)
        .generate()
        .unwrap_err();
    assert!(error.to_string().contains("conflict"), "{error}");
}

#[test]
fn gamma_survivor_service_paths_stream_docs_and_empty_generics() {
    let (empty, outputs) = Generator::new().generate().unwrap();
    assert!(outputs.is_empty());
    let empty = syn::parse2::<syn::File>(empty).unwrap();
    let declaration = empty
        .items
        .iter()
        .find_map(|item| match item {
            syn::Item::Struct(item) if item.ident == "Transcoder" => Some(item),
            _ => None,
        })
        .unwrap();
    assert!(declaration.generics.params.is_empty());
    assert!(declaration.generics.lt_token.is_none());
    assert!(declaration.generics.where_clause.is_none());

    for (request, expected, bridge_expected) in [
        ("Input", "demo :: api :: inner :: Input", "super :: Input"),
        (" super::super::Input", "demo :: Input", "super :: super :: super :: Input"),
        (" crate::Input", "crate :: Input", "crate :: Input"),
        ("::external::Input", ":: external :: Input", ":: external :: Input"),
        ("self::Input", "self :: Input", "super :: Input"),
        ("$crate::Input", "$ crate :: Input", "$ crate :: Input"),
    ] {
        let mut service = ServiceDefinition::new("BookService", Some("first\nsecond\n".into()));
        service.set_module_name("demo.api.inner");
        let rule = HttpRule::new(
            "GetHTTPConfig",
            HttpMethod::GET,
            PathTemplate::parse("/files/{book.id=prefix/**}", Grammar::default()).unwrap(),
        );
        service.add_server_streaming_method(rule, request, "Output", Some("stream first\nstream second".into()));
        service.add_method(
            HttpRule::new("Unary", HttpMethod::POST, PathTemplate::parse("/", Grammar::default()).unwrap()),
            "Input",
            "Output",
            None,
        );
        let (transcoder, output) = Generator::new().add(service).generate().unwrap();
        let rendered = transcoder.to_string();
        assert!(rendered.contains(expected), "{request}: {rendered}");
        assert!(rendered.contains("request . book . get_or_insert_with"), "{rendered}");
        assert!(
            rendered.contains(". id = :: rest_over_grpc :: codegen_helpers :: parse_reserved_path_field"),
            "{rendered}"
        );
        assert!(rendered.contains("enum Route"), "{rendered}");
        assert!(!rendered.contains("pub enum Route"), "{rendered}");
        let bridge = output[0].tonic_bridge().unwrap().to_string();
        assert!(bridge.contains(bridge_expected), "{request}: {bridge}");
        if request.starts_with("$crate") {
            continue;
        }
        let service = syn::parse2::<syn::File>(output[0].service_trait().clone()).unwrap();
        let declaration = service
            .items
            .iter()
            .find_map(|item| match item {
                syn::Item::Trait(item) => Some(item),
                _ => None,
            })
            .unwrap();
        let docs = declaration
            .attrs
            .iter()
            .filter_map(|attr| match &attr.meta {
                syn::Meta::NameValue(value) if value.path.is_ident("doc") => Some(value.value.clone()),
                _ => None,
            })
            .count();
        assert_eq!(docs, 3);
        let methods = declaration
            .items
            .iter()
            .filter_map(|item| match item {
                syn::TraitItem::Fn(item) => Some(item),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(methods[0].sig.ident, "get_http_config");
        assert_eq!(methods[0].attrs.len(), 2);
        assert_eq!(methods[1].attrs.len(), 0);
        let service = output[0].service_trait().to_string();
        assert_eq!(service.matches("ResponseStream").count(), 1);
        assert!(service.contains("stream first") && service.contains("stream second"));
        if !request.starts_with("$crate") {
            let file = syn::parse2::<syn::File>(transcoder).unwrap();
            let declaration = file
                .items
                .iter()
                .find_map(|item| match item {
                    syn::Item::Struct(item) if item.ident == "Transcoder" => Some(item),
                    _ => None,
                })
                .unwrap();
            assert_eq!(declaration.generics.params.len(), 1);
            assert!(declaration.generics.lt_token.is_some());
        }
    }
}

#[test]
fn gamma_survivor_descriptor_missing_docs_and_rooted_extern_paths() {
    let rule = annotation(2, "/v/{book_id}", "", "");
    let descriptor = descriptor_with_comments(&rule, false, false, false);
    let (_, service) = generated(&descriptor, &DescriptorOptions::new());
    assert!(!service.contains("documentation."));
    let partial = descriptor_with_doc_lines(&rule, false, false, Some(" Service documentation. \n"), None);
    let (_, service) = generated(&partial, &DescriptorOptions::new());
    assert!(service.contains("Service documentation."));
    assert!(!service.contains("Method documentation."));
    let blank = descriptor_with_doc_lines(&rule, false, false, Some(" \n "), Some(" \n "));
    let (_, service) = generated(&blank, &DescriptorOptions::new());
    assert!(!service.contains("# [doc ="), "{service}");
    for (rust_path, expected) in [
        (" crate::ExactInput", "crate :: ExactInput"),
        ("::external::Input", ":: external :: Input"),
        ("$crate::Input", "$ crate :: Input"),
    ] {
        let (_, service) = generated(&descriptor, &DescriptorOptions::new().extern_path(".demo.api.Input", rust_path));
        assert!(service.contains(expected), "{rust_path}: {service}");
    }
}

#[cfg(feature = "build-openapi")]
#[test]
fn gamma_survivor_openapi_capture_affixes_and_explicit_absent_extensions() {
    use rest_over_grpc::build::OpenApiInfo;
    let rule = [
        annotation(2, "/v/{book_id=before/middle/*/after/last}", "", ""),
        bytes(11, &annotation(4, "/constant/{book_id=fixed}/{nested.leaf}", "", "")),
    ]
    .concat();
    let services = ServiceDefinition::from_fds(descriptor(&rule, false, false), &DescriptorOptions::new()).unwrap();
    let (_, output) = Generator::builder()
        .emit_openapi_spec(Some(OpenApiInfo::new("Capture", "1")))
        .build()
        .add_all(services)
        .generate()
        .unwrap();
    let spec: serde_json::Value = serde_json::from_str(output[0].openapi_spec().unwrap()).unwrap();
    let operation = &spec["paths"]["/v/before/middle/{book_id}/after/last"]["get"];
    let parameters = operation["parameters"].as_array().unwrap();
    assert_eq!(parameters[0]["x-rest-over-grpc-capture-prefix"], "before/middle/");
    assert_eq!(parameters[0]["x-rest-over-grpc-capture-suffix"], "/after/last");
    for parameter in &parameters[1..] {
        assert!(parameter.get("x-rest-over-grpc-capture-prefix").is_none(), "{parameter}");
        assert!(parameter.get("x-rest-over-grpc-capture-suffix").is_none(), "{parameter}");
    }
    assert_eq!(operation["responses"]["200"]["description"], "Successful response.");
    assert_eq!(
        operation["responses"]["default"]["description"],
        "An error, as a gRPC status mapped to an HTTP status."
    );
    assert_eq!(
        operation["responses"]["200"]["content"]["application/json"]["schema"]["$ref"],
        "#/components/schemas/demo.api.Output"
    );
    let following = &spec["paths"]["/constant/fixed/{nested.leaf}"]["post"]["parameters"];
    assert_eq!(following[0]["name"], "nested.leaf");
}
