// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Inspecting the OpenAPI 3.1 document emitted beside generated REST code.
//!
//! The example build script enables `build-openapi`, configures
//! `GeneratorBuilder::emit_openapi_spec`, and writes
//! `library.openapi.json` into `OUT_DIR/custom`. This example embeds that file
//! and reads the paths a documentation server or packaging step could publish.
//!
//! Run with:
//!
//! ```text
//! cargo run -p rest_over_grpc_examples --example openapi_document
//! ```

use serde_json::Value;

const OPENAPI: &str = include_str!(concat!(env!("OUT_DIR"), "/custom/library.openapi.json"));

fn main() {
    let document: Value = serde_json::from_str(OPENAPI).expect("generated OpenAPI is valid JSON");

    println!(
        "{} {} — OpenAPI {}",
        document["info"]["title"].as_str().unwrap_or(""),
        document["info"]["version"].as_str().unwrap_or(""),
        document["openapi"].as_str().unwrap_or(""),
    );

    let paths = document["paths"].as_object().expect("generated document has paths");
    for (path, operations) in paths {
        let methods = operations
            .as_object()
            .expect("path item is an object")
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(", ");
        println!("{path}: {methods}");
    }
}
