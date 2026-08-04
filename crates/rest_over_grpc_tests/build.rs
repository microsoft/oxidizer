// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Generates the static routers exercised by this crate's benchmark and tests.
//!
//! Both are produced from route tables with `rest_over_grpc::build::generate_router`
//! (no `.proto`, no `prost`): a large GitHub-like table (`bench_routes.rs`) for
//! the `grs_router_vs_matchit` benchmark and its smoke test, and a small table of
//! tricky routing cases for the correctness tests.

use std::path::PathBuf;
use std::{env, fs};

use http_path_template::{Grammar, PathTemplate};
use rest_over_grpc::build::{DescriptorOptions, Generator, HttpRule, OpenApiInfo, ServiceDefinition, compile_fds, generate_router};
use routerama::HttpMethod;

include!("bench_routes.rs");

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set for build scripts"));
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR is set for build scripts"));

    let mut rules = Vec::new();
    for (rpc, method, pattern) in ROUTES {
        let http = http_method(method).expect("known HTTP method in the benchmark route table");
        let template = PathTemplate::parse(pattern, Grammar::default()).expect("benchmark route templates parse cleanly");
        rules.push(HttpRule::new(*rpc, http, template));
    }
    let bench_router = generate_router(rules);
    fs::write(out_dir.join("bench_router.rs"), bench_router.to_string()).expect("writing the benchmark router succeeds");

    let coverage_table: &[(&str, &str, &str)] = &[
        ("SystemConfig", "GET", "/v1/system/config"),
        ("TenantSettings", "GET", "/v1/{tenant}/settings"),
        ("GetBook", "GET", "/v1/books/{book}"),
        ("ArchiveBook", "POST", "/v1/books/{book}:archive"),
        ("GetItem", "GET", "/v1/items/{item.id}"),
        ("GetTree", "GET", "/v1/tree/{path=**}"),
        ("SearchShelf", "GET", "/v1/search/{name=shelves/*}"),
        ("GetX", "GET", "/v1/x"),
        ("GetXY", "GET", "/v1/x/y"),
    ];
    let mut coverage_rules = Vec::new();
    for (rpc, method, pattern) in coverage_table {
        let http = http_method(method).expect("known HTTP method in the coverage route table");
        let template = PathTemplate::parse(pattern, Grammar::default()).expect("coverage route templates parse cleanly");
        coverage_rules.push(HttpRule::new(*rpc, http, template));
    }
    let coverage_router = generate_router(coverage_rules);
    fs::write(out_dir.join("coverage_router.rs"), coverage_router.to_string()).expect("writing the coverage router succeeds");

    // Each pipeline emits `transcoder.rest.rs`, so their output directories must differ.
    let proto_dir = manifest.join("proto");
    build_tonic_bridge(&proto_dir, &out_dir.join("tonic_bridge"));
    build_custom(&proto_dir, &out_dir.join("custom"));

    println!("cargo:rerun-if-changed=bench_routes.rs");
    println!("cargo:rerun-if-changed=proto/greeter.proto");
    println!("cargo:rerun-if-changed=proto/library.proto");
    println!("cargo:rerun-if-changed=proto/google/api/annotations.proto");
    println!("cargo:rerun-if-changed=proto/google/api/http.proto");
    println!("cargo:rerun-if-changed=build.rs");
}

/// The tonic-bridge modality: the greeter service.
///
/// `tonic` generates the messages and the `greeter_server::Greeter` server
/// trait; `rest_over_grpc::build` then emits the REST trait + transcoder and the
/// blanket `tonic` bridge (on by default).
fn build_tonic_bridge(proto_dir: &std::path::Path, out_dir: &std::path::Path) {
    fs::create_dir_all(out_dir).expect("the tonic_bridge output directory is created");

    let mut compiler = protox::Compiler::new([proto_dir]).expect("protox compiler initializes");
    compiler.include_imports(true);
    compiler.open_file("greeter.proto").expect("the greeter proto compiles");
    let descriptor_bytes = compiler.encode_file_descriptor_set();

    tonic_prost_build::configure()
        .build_client(false)
        .build_server(true)
        .build_transport(false)
        .out_dir(out_dir)
        .compile_fds(compiler.file_descriptor_set())
        .expect("tonic generates the messages and server trait");

    pbjson_build::Builder::new()
        .register_descriptors(&descriptor_bytes)
        .expect("pbjson registers the descriptors")
        .out_dir(out_dir)
        .build(&[".greeter"])
        .expect("pbjson generates the serde impls");

    compile_fds(&descriptor_bytes, out_dir).expect("the http annotations decode and the generated REST service code is written");
}

/// The custom-handler modality: the library service.
///
/// `prost` generates only the message structs (no server); the generated REST
/// service trait is implemented directly, so the `tonic` bridge is disabled. An
/// OpenAPI 3.1 document is emitted alongside describing the transcoded surface.
fn build_custom(proto_dir: &std::path::Path, out_dir: &std::path::Path) {
    fs::create_dir_all(out_dir).expect("the custom output directory is created");

    let mut compiler = protox::Compiler::new([proto_dir]).expect("protox compiler initializes");
    compiler.include_imports(true);
    compiler.include_source_info(true);
    compiler.open_file("library.proto").expect("the example proto compiles");
    let descriptor_bytes = compiler.encode_file_descriptor_set();

    prost_build::Config::new()
        .out_dir(out_dir)
        .compile_fds(compiler.file_descriptor_set())
        .expect("prost generates the message structs");

    pbjson_build::Builder::new()
        .register_descriptors(&descriptor_bytes)
        .expect("pbjson registers the descriptors")
        .out_dir(out_dir)
        .build(&[".library"])
        .expect("pbjson generates the serde impls");

    Generator::builder()
        .emit_tonic_bridge(false)
        .emit_openapi_spec(Some(OpenApiInfo::new("Library", "v1")))
        .build()
        .add_all(
            ServiceDefinition::from_fds(&descriptor_bytes, &DescriptorOptions::new().package(".library"))
                .expect("the http annotations decode"),
        )
        .write(out_dir)
        .expect("the generated REST service code is written");
}

/// Maps an uppercase HTTP method name from a route table to an [`HttpMethod`],
/// returning `None` for an unrecognized method.
fn http_method(method: &str) -> Option<HttpMethod> {
    Some(match method {
        "GET" => HttpMethod::GET,
        "PUT" => HttpMethod::PUT,
        "POST" => HttpMethod::POST,
        "DELETE" => HttpMethod::DELETE,
        "PATCH" => HttpMethod::PATCH,
        _ => return None,
    })
}
