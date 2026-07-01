// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Compiles the sample `.proto` (messages + pbjson serde) and generates the
//! REST service trait + dispatcher by reading the `google.api.http` annotations
//! straight from the proto, exercising the full `rest_over_grpc_build` → `rest_over_grpc`
//! pipeline end to end.

use std::path::PathBuf;
use std::{env, fs};

use rest_over_grpc_build::{HttpMethod, HttpRule, Router, services_from_descriptor, write_annotation_protos};

// The large benchmark route table, shared with `benches/router_vs_axum.rs`.
include!("bench_routes.rs");

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set for build scripts"));
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR is set for build scripts"));
    let proto_dir = manifest.join("proto");

    // Materialize the vendored google.api annotation protos so the proto's
    // `import "google/api/annotations.proto"` resolves.
    let annotations_include = out_dir.join("proto_include");
    write_annotation_protos(&annotations_include).expect("vendored annotation protos are written");

    // 1. Parse the proto into a descriptor set (pure-Rust, no `protoc`).
    //    `encode_file_descriptor_set` preserves custom options (the http
    //    annotation), which a round-trip through `prost_types` would drop.
    let mut compiler = protox::Compiler::new([&proto_dir, &annotations_include]).expect("protox compiler initializes");
    compiler.include_imports(true);
    compiler.open_file("library.proto").expect("the sample proto compiles");
    let descriptor_bytes = compiler.encode_file_descriptor_set();

    // 2. Generate the prost message structs into OUT_DIR (`library.rs`).
    prost_build::Config::new()
        .compile_fds(compiler.file_descriptor_set())
        .expect("prost generates the message structs");

    // 3. Generate the pbjson serde impls into OUT_DIR (`library.serde.rs`).
    pbjson_build::Builder::new()
        .register_descriptors(&descriptor_bytes)
        .expect("pbjson registers the descriptors")
        .build(&[".library"])
        .expect("pbjson generates the serde impls");

    // 4. Read the google.api.http annotations and generate the REST service
    //    trait + dispatcher into OUT_DIR (`router.rs`).
    let services = services_from_descriptor(&descriptor_bytes, "crate::pb").expect("the http annotations are read");
    assert_eq!(services.len(), 1, "the sample defines exactly one service");
    let code = services[0].generate();
    fs::write(out_dir.join("router.rs"), code.to_string()).expect("writing the generated router succeeds");

    // 5. Generate the large benchmark router (`bench_router.rs`) from the shared
    //    route table, so `benches/router_vs_axum.rs` can compare the generated
    //    static router against `axum`/`matchit` built from the same routes.
    let mut routes = Vec::new();
    for (rpc, method, pattern) in ROUTES {
        let http = http_method(method).expect("known HTTP method in the benchmark route table");
        let rule = HttpRule::new(*rpc, http, *pattern);
        routes.extend(rule.lower().expect("benchmark route templates lower cleanly"));
    }
    let bench_router = Router::new(routes).generate();
    fs::write(out_dir.join("bench_router.rs"), bench_router.to_string()).expect("writing the benchmark router succeeds");

    // 6. Generate a small router that exercises tricky routing cases (literal vs
    //    wildcard backtracking, custom verbs, nested field paths, `**` capture,
    //    prefix overlap) so the runtime behavior of the generated trie is tested.
    let coverage_table: &[(&str, &str, &str)] = &[
        ("SystemConfig", "GET", "/v1/system/config"),
        ("TenantSettings", "GET", "/v1/{tenant}/settings"),
        ("GetBook", "GET", "/v1/books/{book}"),
        ("ArchiveBook", "POST", "/v1/books/{book}:archive"),
        ("GetItem", "GET", "/v1/items/{item.id}"),
        ("GetTree", "GET", "/v1/tree/{path=**}"),
        ("GetX", "GET", "/v1/x"),
        ("GetXY", "GET", "/v1/x/y"),
    ];
    let mut coverage_lowered = Vec::new();
    for (rpc, method, pattern) in coverage_table {
        let http = http_method(method).expect("known HTTP method in the coverage route table");
        coverage_lowered.extend(
            HttpRule::new(*rpc, http, *pattern)
                .lower()
                .expect("coverage route templates lower cleanly"),
        );
    }
    let coverage_router = Router::new(coverage_lowered).generate();
    fs::write(out_dir.join("coverage_router.rs"), coverage_router.to_string()).expect("writing the coverage router succeeds");

    println!("cargo:rerun-if-changed=proto/library.proto");
    println!("cargo:rerun-if-changed=bench_routes.rs");
    println!("cargo:rerun-if-changed=build.rs");
}

/// Maps an uppercase HTTP method name from the route table to an [`HttpMethod`],
/// returning `None` for an unrecognized method.
fn http_method(method: &str) -> Option<HttpMethod> {
    Some(match method {
        "GET" => HttpMethod::Get,
        "PUT" => HttpMethod::Put,
        "POST" => HttpMethod::Post,
        "DELETE" => HttpMethod::Delete,
        "PATCH" => HttpMethod::Patch,
        _ => return None,
    })
}
