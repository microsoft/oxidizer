// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Generating a REST service from `google.api.http` rules.
//!
//! Builds [`HttpRule`]s (normally read straight from proto annotations by
//! [`ServiceDefinition::from_fds`](rest_over_grpc::build::ServiceDefinition::from_fds)),
//! registers them on a [`ServiceDefinition`], and generates the framework-neutral
//! service trait + request/response transcoder as a `TokenStream`. A consumer's
//! `build.rs` writes this to `OUT_DIR` and `include!`s it; here we pretty-print it
//! to stdout.
//!
//! Run with:
//!
//! ```text
//! cargo run -p rest_over_grpc --example generate_service --features build
//! ```

use rest_over_grpc::build::{Generator, HttpMethod, HttpRule, RequestBody, ServiceDefinition};

fn main() {
    // Each RPC's HTTP binding. `GetShelf` captures the `{shelf}` path variable;
    // `CreateShelf` maps the request body onto the request message's `shelf`
    // field.
    let get_shelf = HttpRule::new(
        "GetShelf",
        HttpMethod::Get,
        "/v1/shelves/{shelf}".parse().expect("valid path template"),
    );
    let create_shelf = HttpRule::new("CreateShelf", HttpMethod::Post, "/v1/shelves".parse().expect("valid path template"))
        .request_body(RequestBody::Field("shelf".to_owned()));

    // Register each RPC's binding with its request/response Rust type paths
    // (where the `prost`-generated messages live).
    let mut library = ServiceDefinition::new("Library", None);
    library
        .add_method(get_shelf, "crate::pb::GetShelfRequest", "crate::pb::Shelf", None)
        .add_method(create_shelf, "crate::pb::CreateShelfRequest", "crate::pb::Shelf", None);

    let (transcoder, generated) = Generator::new().add(library).generate();

    // Render the generated Rust for display; a build script would instead
    // write the code to files under `OUT_DIR` with `Generator::write`.
    for service in generated {
        let mut code = service.r#trait().clone();
        if let Some(bridge) = service.tonic_bridge() {
            code.extend(bridge.clone());
        }
        let file = syn::parse2(code).expect("generated code is valid Rust");
        println!("{}", prettyplease::unparse(&file));
    }

    // The top-level transcoder that routes across all services.
    let file = syn::parse2(transcoder).expect("generated transcoder is valid Rust");
    println!("{}", prettyplease::unparse(&file));
}
