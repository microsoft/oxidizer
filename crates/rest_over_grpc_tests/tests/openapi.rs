// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Verifies the OpenAPI 3.1 document emitted by `rest_over_grpc::build` at build
//! time (step 4 of `build.rs`) describes the example service's REST surface.

use serde_json::Value;

/// The spec generated into `OUT_DIR` alongside `library.rest.rs`.
const SPEC: &str = include_str!(concat!(env!("OUT_DIR"), "/custom/library.openapi.json"));

fn spec() -> Value {
    serde_json::from_str(SPEC).expect("generated OpenAPI is valid JSON")
}

#[test]
fn document_is_openapi_31_with_info() {
    let doc = spec();
    assert_eq!(doc["openapi"], "3.1.0");
    assert_eq!(doc["info"]["title"], "Library");
    assert_eq!(doc["info"]["version"], "v1");
}

#[test]
fn every_rpc_binding_is_a_path() {
    let doc = spec();
    let paths = doc["paths"].as_object().expect("paths object");

    assert_eq!(paths["/v1/shelves/{shelf}"]["get"]["operationId"], "GetShelf");
    assert_eq!(paths["/v1/shelves"]["post"]["operationId"], "CreateShelf");
    assert_eq!(paths["/v1/shelves"]["get"]["operationId"], "ListShelves");
    assert_eq!(paths["/v1/shelves:stream"]["get"]["operationId"], "StreamShelves");
}

#[test]
fn reserved_multi_segment_capture_is_reported_without_a_misleading_path() {
    let doc = spec();
    let paths = doc["paths"].as_object().expect("paths object");
    assert!(
        !paths.contains_key("/v1/tree/{path}"),
        "multi-segment capture is not a single path parameter"
    );
    assert_eq!(paths["/v1/shelves/{shelf}"]["get"]["operationId"], "GetShelf");
    let omitted = doc["x-rest-over-grpc-omitted-operations"]
        .as_array()
        .expect("explicit omitted operations");
    let tree = omitted
        .iter()
        .find(|operation| operation["operationId"] == "GetTree")
        .expect("GetTree omission");
    assert_eq!(tree["method"], "get");
    assert_eq!(tree["path"], "/v1/tree/{path=**}");
    assert!(tree["reason"].as_str().unwrap().contains("multiple segments"));
}

#[test]
fn fixed_prefix_capture_resolves_to_the_runtime_route() {
    use rest_over_grpc_tests::coverage::Route;

    assert_eq!(
        Route::resolve("GET", "/v1/lookup/items/42"),
        Some(Route::GetFixedPrefix { name: "items/42" })
    );
    assert!(Route::resolve("GET", "/v1/lookup/other/42").is_none());
}

#[test]
fn path_parameter_and_query_parameter_are_described() {
    let doc = spec();

    let get_shelf = &doc["paths"]["/v1/shelves/{shelf}"]["get"]["parameters"][0];
    assert_eq!(get_shelf["name"], "shelf");
    assert_eq!(get_shelf["in"], "path");
    assert_eq!(get_shelf["required"], true);

    let list = doc["paths"]["/v1/shelves"]["get"]["parameters"].as_array().expect("query params");
    let filter = list.iter().find(|p| p["name"] == "filter").expect("filter query param");
    assert_eq!(filter["in"], "query");
    assert_eq!(filter["schema"]["type"], "string");
}

#[test]
fn create_shelf_binds_the_shelf_body_field() {
    let doc = spec();
    let schema = &doc["paths"]["/v1/shelves"]["post"]["requestBody"]["content"]["application/json"]["schema"];
    assert_eq!(schema["$ref"], "#/components/schemas/library.Shelf");
}

#[test]
fn streaming_rpc_returns_an_array_of_shelves() {
    let doc = spec();
    let schema = &doc["paths"]["/v1/shelves:stream"]["get"]["responses"]["200"]["content"]["application/json"]["schema"];
    assert_eq!(schema["type"], "array");
    assert_eq!(schema["items"]["$ref"], "#/components/schemas/library.Shelf");
}

#[test]
fn message_schemas_are_components() {
    let doc = spec();
    let shelf = &doc["components"]["schemas"]["library.Shelf"];
    assert_eq!(shelf["type"], "object");
    assert_eq!(shelf["properties"]["name"]["type"], "string");
    assert_eq!(shelf["properties"]["theme"]["type"], "string");
    assert_eq!(doc["components"]["schemas"]["google.rpc.Status"]["type"], "object");
}
