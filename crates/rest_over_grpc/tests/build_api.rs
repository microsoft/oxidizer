// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration tests for the public `rest_over_grpc::build` API: assembling
//! service definitions, generating/writing code, the descriptor pipeline, and
//! the smaller building blocks.

#![cfg(all(feature = "build", not(miri)))] // filesystem I/O is unsupported under Miri.

use http_path_template::{Grammar, PathTemplate};
use rest_over_grpc::build::{Binding, Generator, HttpMethod, HttpRule, ResponseBody, ServiceDefinition, generate_router};

fn rule(rpc: &str, method: HttpMethod, pattern: &str) -> HttpRule {
    HttpRule::new(
        rpc,
        method,
        PathTemplate::parse(pattern, Grammar::default()).expect("valid path template"),
    )
}

#[test]
fn generator_renders_and_inspects_services() {
    let mut library = ServiceDefinition::new("Library", None);
    library.add_method(
        rule("GetShelf", HttpMethod::GET, "/v1/shelves/{shelf}"),
        "crate::pb::GetShelfRequest",
        "crate::pb::Shelf",
        None,
    );

    let mut generator = Generator::new();
    generator.add(library).add_all([ServiceDefinition::new("Empty", None)]);

    let (transcoder, generated) = generator.generate();
    assert_eq!(generated.len(), 2);
    // The transcoder is always produced and routes across the added services.
    assert!(transcoder.to_string().contains("struct Transcoder"));
    let library = generated.iter().find(|g| g.trait_name() == "Library").expect("Library generated");
    assert_eq!(library.module_name(), "library");
    assert!(library.r#trait().to_string().contains("get_shelf"));
    // The tonic bridge is on by default; the trait itself carries no bridge impl.
    assert!(library.tonic_bridge().is_some());
    assert!(library.r#trait().to_string().contains("pub trait Library"));
}

#[test]
fn generator_writes_one_file_per_module() {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join(format!("rog_build_it_write_{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("scratch dir");

    let mut library = ServiceDefinition::new("Library", None);
    library.add_method(
        rule("GetShelf", HttpMethod::GET, "/v1/shelves/{shelf}"),
        "crate::pb::GetShelfRequest",
        "crate::pb::Shelf",
        None,
    );
    Generator::new().add(library).write(&dir).expect("writes generated code");

    let written = std::fs::read_to_string(dir.join("library.rest.rs")).expect("output file exists");
    assert!(written.contains("trait Library"));
}

#[test]
fn generator_concatenates_services_sharing_a_module() {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join(format!("rog_build_it_shared_{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("scratch dir");

    // Two services grouped into the same module are concatenated into one file.
    let mut books = ServiceDefinition::new("Books", None);
    books.module("catalog").add_method(
        rule("GetBook", HttpMethod::GET, "/v1/books/{book}"),
        "crate::Req",
        "crate::Resp",
        None,
    );
    let mut shelves = ServiceDefinition::new("Shelves", None);
    shelves.module("catalog").add_method(
        rule("GetShelf", HttpMethod::GET, "/v1/shelves/{shelf}"),
        "crate::Req",
        "crate::Resp",
        None,
    );

    Generator::new().add(books).add(shelves).write(&dir).expect("writes generated code");

    let written = std::fs::read_to_string(dir.join("catalog.rest.rs")).expect("shared module file exists");
    assert!(written.contains("trait Books"));
    assert!(written.contains("trait Shelves"));
}

#[test]
fn service_definition_name_and_module_default_from_trait() {
    let definition = ServiceDefinition::new("BookService", None);
    assert_eq!(definition.trait_name(), "BookService");
    assert_eq!(definition.module_name(), "book_service");
}

#[test]
fn binding_exposes_method_and_template() {
    let binding = Binding::new(
        HttpMethod::POST,
        PathTemplate::parse("/v1/shelves/{shelf}:archive", Grammar::default()).expect("valid"),
    )
    .response_body(ResponseBody::Field("shelf".to_owned()));
    assert_eq!(binding.method(), &HttpMethod::POST);
    assert_eq!(binding.template().verb(), Some("archive"));
}

#[test]
fn generate_router_is_standalone_and_covers_additional_bindings() {
    let rule = rule("GetShelf", HttpMethod::GET, "/v1/shelves/{shelf}").add_binding(Binding::new(
        HttpMethod::GET,
        PathTemplate::parse("/v1/libraries/{shelf}", Grammar::default()).expect("valid"),
    ));
    let code = generate_router([rule]).to_string();
    assert!(code.contains("GetShelf"));
    assert!(code.contains("libraries"));
}

#[cfg(feature = "build")]
mod descriptor {
    #[cfg(feature = "build-openapi")]
    use http_path_template::{Grammar, PathTemplate};
    #[cfg(feature = "build-openapi")]
    use rest_over_grpc::build::Generator;
    use rest_over_grpc::build::{DescriptorOptions, ServiceDefinition, compile_fds};

    /// Compiles an inline proto (with the vendored `google.api` annotations) into
    /// an encoded `FileDescriptorSet`, mirroring what a `build.rs` produces.
    fn descriptor_set(source: &str) -> Vec<u8> {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let unique = NEXT.fetch_add(1, Ordering::Relaxed);

        let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let scratch = manifest
            .join("target")
            .join(format!("rog_build_it_desc_{}_{unique}", std::process::id()));
        std::fs::create_dir_all(&scratch).expect("scratch dir");
        std::fs::write(scratch.join("api.proto"), source).expect("write proto");

        let annotations = manifest.join("tests").join("proto");
        let mut compiler = protox::Compiler::new([scratch.as_path(), annotations.as_path()]).expect("compiler");
        compiler.include_imports(true);
        compiler.open_file("api.proto").expect("proto compiles");
        compiler.encode_file_descriptor_set()
    }

    const PROTO: &str = r#"
        syntax = "proto3";
        package api;
        import "google/api/annotations.proto";
        message GetThingRequest { string id = 1; }
        message Thing { string id = 1; }
        service ThingService {
            rpc GetThing(GetThingRequest) returns (Thing) {
                option (google.api.http) = { get: "/v1/things/{id}" };
            }
        }
    "#;

    #[test]
    fn from_fds_decodes_annotated_services() {
        let descriptor = descriptor_set(PROTO);
        let options = DescriptorOptions::new()
            .package(".api")
            .extern_path(".google.protobuf.Empty", "::prost_types::Empty");
        let services = ServiceDefinition::from_fds(&descriptor, &options).expect("services decode");
        assert_eq!(services.len(), 1);
        assert_eq!(services[0].trait_name(), "ThingService");
    }

    #[test]
    fn compile_fds_writes_generated_code() {
        let descriptor = descriptor_set(PROTO);
        let out = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(format!("rog_build_it_out_{}", std::process::id()));
        std::fs::create_dir_all(&out).expect("out dir");

        compile_fds(&descriptor, &out).expect("compiles and writes");
        let written = std::fs::read_to_string(out.join("api.rest.rs")).expect("output file exists");
        assert!(written.contains("trait ThingService"));
    }

    #[test]
    fn compile_fds_reports_a_write_error() {
        let descriptor = descriptor_set(PROTO);
        // A path whose parent does not exist cannot be written to.
        let bad = std::path::PathBuf::from("/rest_over_grpc_codegen_nonexistent_dir/nested");
        let error = compile_fds(&descriptor, &bad).expect_err("write fails");
        assert!(error.to_string().contains("failed to write"));
        assert!(std::error::Error::source(&error).is_some());
    }

    #[cfg(feature = "build-openapi")]
    #[test]
    fn write_emits_openapi_document_for_named_package() {
        use rest_over_grpc::build::OpenApiInfo;

        let descriptor = descriptor_set(PROTO);
        let out = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(format!("rog_build_it_openapi_named_{}", std::process::id()));
        std::fs::create_dir_all(&out).expect("out dir");

        Generator::builder()
            .emit_tonic_bridge(false)
            .emit_openapi_spec(Some(OpenApiInfo::new("Things API", "v1")))
            .build()
            .add_all(ServiceDefinition::from_fds(&descriptor, &DescriptorOptions::new().package(".api")).expect("decode"))
            .write(&out)
            .expect("writes");

        // Both the code and the spec are written, the spec named after the module.
        assert!(out.join("api.rest.rs").exists(), "code still written");
        let spec = std::fs::read_to_string(out.join("api.openapi.json")).expect("spec file exists");
        assert!(spec.contains("\"openapi\": \"3.1.0\""), "{spec}");
        assert!(spec.contains("/v1/things/{id}"));
        assert!(spec.contains("Things API"));
    }

    #[cfg(feature = "build-openapi")]
    #[test]
    fn write_emits_openapi_document_for_default_package() {
        use rest_over_grpc::build::OpenApiInfo;

        const NO_PACKAGE: &str = r#"
            syntax = "proto3";
            import "google/api/annotations.proto";
            message R { string id = 1; }
            service S {
                rpc G(R) returns (R) { option (google.api.http) = { get: "/x/{id}" }; }
            }
        "#;
        let descriptor = descriptor_set(NO_PACKAGE);
        let out = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(format!("rog_build_it_openapi_default_{}", std::process::id()));
        std::fs::create_dir_all(&out).expect("out dir");

        Generator::builder()
            .emit_tonic_bridge(false)
            .emit_openapi_spec(Some(OpenApiInfo::new("Default", "v1")))
            .build()
            .add_all(ServiceDefinition::from_fds(&descriptor, &DescriptorOptions::new()).expect("decode"))
            .write(&out)
            .expect("writes");

        // With no proto package the module defaults to the snake-cased trait name,
        // so the spec sits beside `s.rest.rs` as `s.openapi.json`.
        assert!(out.join("s.openapi.json").exists(), "default-package spec written");
    }

    #[cfg(feature = "build-openapi")]
    #[test]
    fn write_merges_openapi_documents_for_services_sharing_a_package() {
        use rest_over_grpc::build::OpenApiInfo;

        // Two services in one package merge into one `{package}.openapi.json`
        // rather than clobbering each other.
        const TWO_SERVICES: &str = r#"
            syntax = "proto3";
            package shared;
            import "google/api/annotations.proto";
            message R { string id = 1; }
            service A { rpc GetA(R) returns (R) { option (google.api.http) = { get: "/v1/a/{id}" }; } }
            service B { rpc GetB(R) returns (R) { option (google.api.http) = { get: "/v1/b/{id}" }; } }
        "#;
        let descriptor = descriptor_set(TWO_SERVICES);
        let out = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(format!("rog_build_it_openapi_merge_{}", std::process::id()));
        std::fs::create_dir_all(&out).expect("out dir");

        Generator::builder()
            .emit_tonic_bridge(false)
            .emit_openapi_spec(Some(OpenApiInfo::new("Shared", "v1")))
            .build()
            .add_all(ServiceDefinition::from_fds(&descriptor, &DescriptorOptions::new()).expect("decode"))
            .write(&out)
            .expect("writes");

        let spec = std::fs::read_to_string(out.join("shared.openapi.json")).expect("merged spec exists");
        assert!(spec.contains("/v1/a/{id}"), "service A path present: {spec}");
        assert!(spec.contains("/v1/b/{id}"), "service B path present: {spec}");
    }

    #[cfg(feature = "build-openapi")]
    #[test]
    fn generate_exposes_the_openapi_spec_per_service() {
        use rest_over_grpc::build::{HttpMethod, HttpRule, OpenApiInfo};

        let descriptor = descriptor_set(PROTO);

        // With OpenAPI requested, `openapi_spec()` carries the per-service document.
        let mut with_spec = Generator::builder()
            .emit_openapi_spec(Some(OpenApiInfo::new("Things API", "v1")))
            .build();
        with_spec.add_all(ServiceDefinition::from_fds(&descriptor, &DescriptorOptions::new().package(".api")).expect("decode"));
        let generated = with_spec.generate().1;
        let spec = generated[0]
            .openapi_spec()
            .expect("descriptor-decoded service carries an OpenAPI spec");
        assert!(spec.contains("\"openapi\": \"3.1.0\""), "{spec}");
        assert!(spec.contains("/v1/things/{id}"));
        assert!(spec.contains("Things API"));

        // Without OpenAPI requested, a decoded service still has no spec.
        let mut no_spec = Generator::new();
        no_spec.add_all(ServiceDefinition::from_fds(&descriptor, &DescriptorOptions::new().package(".api")).expect("decode"));
        assert!(no_spec.generate().1[0].openapi_spec().is_none());

        // A hand-built service has no descriptor, so no OpenAPI state even when requested.
        let mut manual = Generator::builder()
            .emit_openapi_spec(Some(OpenApiInfo::new("Manual", "v1")))
            .build();
        let mut service = ServiceDefinition::new("Manual", None);
        service.add_method(
            HttpRule::new(
                "Get",
                HttpMethod::GET,
                PathTemplate::parse("/v1/x/{id}", Grammar::default()).expect("valid path template"),
            ),
            "crate::Req",
            "crate::Resp",
            None,
        );
        manual.add(service);
        assert!(manual.generate().1[0].openapi_spec().is_none());
    }
}
