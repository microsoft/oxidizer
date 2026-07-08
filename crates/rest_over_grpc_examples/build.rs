// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Generates the code for both example modalities into separate `OUT_DIR`
//! subdirectories (each pipeline emits a top-level `transcoder.rest.rs`, so they
//! must not share a directory):
//!
//! - `tonic_bridge/` — `library.proto` built with `tonic` (messages + server
//!   trait) + pbjson serde + the `rest_over_grpc::build` REST trait, transcoder,
//!   and the blanket `tonic` bridge. See `src/tonic_bridge.rs`.
//! - `custom/` — `library.proto` built with `prost` (messages only) + pbjson
//!   serde + the `rest_over_grpc::build` REST trait and transcoder (no `tonic`
//!   bridge), plus an OpenAPI 3.1 document. See `src/custom.rs`.

use std::env;
use std::path::PathBuf;

use rest_over_grpc::build::{DescriptorOptions, Generator, OpenApiInfo, ServiceDefinition, compile_fds};

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set for build scripts"));
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR is set for build scripts"));
    let proto_dir = manifest.join("proto");

    build_tonic_bridge(&proto_dir, &out_dir.join("tonic_bridge"));
    build_custom(&proto_dir, &out_dir.join("custom"));

    println!("cargo:rerun-if-changed=proto/library.proto");
    println!("cargo:rerun-if-changed=build.rs");
}

/// The tonic-bridge modality.
///
/// `tonic` generates the messages and the `library_server::Library` server
/// trait; `rest_over_grpc::build` then emits the REST trait + transcoder and —
/// because the `tonic` bridge is on by default — the blanket bridge
/// `impl <Library> for T where T: library_server::Library`, so a service written
/// once against `tonic` also serves REST.
fn build_tonic_bridge(proto_dir: &std::path::Path, out_dir: &std::path::Path) {
    std::fs::create_dir_all(out_dir).expect("the tonic_bridge output directory is created");

    // Compile the proto to a descriptor set with protox (pure Rust, no protoc).
    let mut compiler = protox::Compiler::new([proto_dir]).expect("protox compiler initializes");
    compiler.include_imports(true);
    compiler.open_file("library.proto").expect("the library proto compiles");
    let descriptor_bytes = compiler.encode_file_descriptor_set();

    // tonic generates the message structs AND the `library_server::Library`
    // server trait — no protoc run. The transport server struct is not needed
    // (the bridge targets only the trait), so it is disabled to keep the
    // dependency surface minimal.
    tonic_prost_build::configure()
        .build_client(false)
        .build_server(true)
        .build_transport(false)
        .out_dir(out_dir)
        .compile_fds(compiler.file_descriptor_set())
        .expect("tonic generates the messages and server trait");

    // pbjson adds the serde impls the JSON transcoder needs.
    pbjson_build::Builder::new()
        .register_descriptors(&descriptor_bytes)
        .expect("pbjson registers the descriptors")
        .out_dir(out_dir)
        .build(&[".library"])
        .expect("pbjson generates the serde impls");

    // `rest_over_grpc::build` emits the REST trait + transcoder and, because the
    // `tonic` bridge is on by default, the blanket bridge impl. The default
    // `Send` bounds match tonic's `#[async_trait]` server trait.
    compile_fds(&descriptor_bytes, out_dir).expect("the http annotations decode and the generated REST service code is written");
}

/// The custom-handler modality: the library service.
///
/// `prost` generates only the message structs (no server); the generated REST
/// service trait is implemented directly, so the `tonic` bridge is disabled. An
/// OpenAPI 3.1 document is emitted alongside describing the transcoded surface.
fn build_custom(proto_dir: &std::path::Path, out_dir: &std::path::Path) {
    std::fs::create_dir_all(out_dir).expect("the custom output directory is created");

    // Parse the proto into a descriptor set (pure-Rust, no `protoc`).
    // `encode_file_descriptor_set` preserves custom options (the http
    // annotation), which a round-trip through `prost_types` would drop.
    let mut compiler = protox::Compiler::new([proto_dir]).expect("protox compiler initializes");
    compiler.include_imports(true);
    // Retain leading comments so each RPC's proto documentation is carried onto
    // the generated trait method.
    compiler.include_source_info(true);
    compiler.open_file("library.proto").expect("the example proto compiles");
    let descriptor_bytes = compiler.encode_file_descriptor_set();

    // Generate the prost message structs.
    prost_build::Config::new()
        .out_dir(out_dir)
        .compile_fds(compiler.file_descriptor_set())
        .expect("prost generates the message structs");

    // Generate the pbjson serde impls.
    pbjson_build::Builder::new()
        .register_descriptors(&descriptor_bytes)
        .expect("pbjson registers the descriptors")
        .out_dir(out_dir)
        .build(&[".library"])
        .expect("pbjson generates the serde impls");

    // Read the google.api.http annotations and generate the REST service trait +
    // transcoder, reusing the descriptor already compiled above.
    // `emit_openapi_spec(..)` additionally makes `write` emit a
    // `library.openapi.json` describing the transcoded REST surface as an OpenAPI
    // 3.1 document. The `tonic` bridge is disabled because this modality uses
    // `prost` (not `tonic`), so there is no `tonic`-generated server trait to
    // bridge.
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
