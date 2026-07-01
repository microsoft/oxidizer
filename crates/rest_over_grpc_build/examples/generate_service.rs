// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Generating a REST service from `google.api.http` rules.
//!
//! Builds [`HttpRule`]s (normally read straight from proto annotations by
//! [`services_from_descriptor`](rest_over_grpc_build::services_from_descriptor)),
//! lowers them into routes, and generates the framework-neutral service trait +
//! request/response dispatcher as a `TokenStream`. A consumer's `build.rs` writes
//! this to `OUT_DIR` and `include!`s it; here we pretty-print it to stdout.
//!
//! Run with:
//!
//! ```text
//! cargo run -p rest_over_grpc_build --example generate_service
//! ```

use rest_over_grpc_build::{Body, HttpMethod, HttpRule, Service, ServiceMethod};

fn main() {
    // Each RPC's HTTP binding. `GetShelf` captures the `{shelf}` path variable;
    // `CreateShelf` maps the request body onto the request message's `shelf`
    // field.
    let get_shelf = HttpRule::new("GetShelf", HttpMethod::Get, "/v1/shelves/{shelf}");
    let create_shelf = HttpRule::new("CreateShelf", HttpMethod::Post, "/v1/shelves").with_body(Body::Field("shelf".to_owned()));

    // Pair each RPC with its request/response Rust type paths (where the
    // `prost`-generated messages live) and the routes lowered from its rule.
    let methods = vec![
        ServiceMethod::new(
            "GetShelf",
            ("crate::pb::GetShelfRequest", "crate::pb::Shelf"),
            get_shelf.lower().expect("GetShelf rule lowers"),
        ),
        ServiceMethod::new(
            "CreateShelf",
            ("crate::pb::CreateShelfRequest", "crate::pb::Shelf"),
            create_shelf.lower().expect("CreateShelf rule lowers"),
        ),
    ];

    let code = Service::new("Library", methods).generate();

    // Render the generated Rust for display; a build script would instead
    // `fs::write` `code.to_string()` to a file under `OUT_DIR`.
    let file = syn::parse2(code).expect("generated code is valid Rust");
    println!("{}", prettyplease::unparse(&file));
}
