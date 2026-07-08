// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The code generator that produces the REST API layer.
//!
//! This module is only available when compiling with the `build` Cargo feature.
//!
//! Both the REST layer and the original gRPC API live in the same process, so
//! the REST layer adds minimal overhead: your service implements only the gRPC
//! API, yet serves both a legacy REST API and a modern gRPC API. There are two
//! ways to generate the layer — from protobuf file descriptors (the common
//! case) or by hand — and it can additionally emit an OpenAPI spec.
//!
//! # Starting from `.proto` files
//!
//! The usual path drives generation from a `FileDescriptorSet` in `build.rs`, in
//! four steps: compile the `.proto`, generate the `prost` message structs,
//! generate the `pbjson` proto3-canonical `serde` impls the JSON transcoding is
//! built on, then read the `google.api.http` annotations and generate the REST
//! layer. The [`compile_fds`] one-liner covers the common
//! default; the expanded form gives control over each step:
//!
//! ```ignore
//! // build.rs
//! use rest_over_grpc::build::{DescriptorOptions, Generator, ServiceDefinition};
//!
//! fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let out_dir = std::path::PathBuf::from(std::env::var("OUT_DIR")?);
//!
//!     // 1. Compile the `.proto` to a `FileDescriptorSet`. `include_source_info`
//!     //    retains the proto comments so they document the generated methods.
//!     let mut compiler = protox::Compiler::new(["proto"])?;
//!     compiler.include_imports(true);
//!     compiler.include_source_info(true);
//!     compiler.open_file("library.proto")?;
//!     let descriptor_set = compiler.encode_file_descriptor_set();
//!
//!     // 2. Generate the `prost` message structs (into `$OUT_DIR/library.rs`).
//!     prost_build::Config::new().compile_fds(compiler.file_descriptor_set())?;
//!
//!     // 3. Generate the `pbjson` proto3-canonical `serde` impls for those
//!     //    messages (into `$OUT_DIR/library.serde.rs`).
//!     pbjson_build::Builder::new()
//!         .register_descriptors(&descriptor_set)?
//!         .build(&[".library"])?;
//!
//!     // 4. Read the `google.api.http` annotations from the same descriptor set
//!     //    and generate the REST service trait (into `$OUT_DIR/library.rest.rs`)
//!     //    plus the top-level `Transcoder` (into `$OUT_DIR/transcoder.rest.rs`).
//!     //    The `tonic` bridge is on by default; this `prost`-based build has no
//!     //    `tonic`-generated server to bridge, so it is disabled.
//!     Generator::builder()
//!         .emit_tonic_bridge(false)
//!         .build()
//!         .add_all(ServiceDefinition::from_fds(&descriptor_set, &DescriptorOptions::new().package(".library"))?)
//!         .write(&out_dir)?;
//!
//!     Ok(())
//! }
//! ```
//!
//! Then `include!` the per-package files into a module named after the proto
//! package, so the message types, their `serde` impls, and the service trait
//! resolve together. `include!` `transcoder.rest.rs` at the enclosing scope,
//! where it refers to each service by its module-qualified path
//! (`library::Library`):
//!
//! ```ignore
//! pub mod library {
//!     include!(concat!(env!("OUT_DIR"), "/library.rs"));
//!     include!(concat!(env!("OUT_DIR"), "/library.serde.rs"));
//!     include!(concat!(env!("OUT_DIR"), "/library.rest.rs"));
//! }
//!
//! include!(concat!(env!("OUT_DIR"), "/transcoder.rest.rs")); // defines `Transcoder`
//! ```
//!
//! # Manual construction
//!
//! When you need more control — a package filter, disabling the `tonic` bridge,
//! OpenAPI output, or when you have no `.proto` at all — build a
//! [`ServiceDefinition`] (by hand or via
//! [`ServiceDefinition::from_fds`]) and
//! emit code with a [`Generator`] (configured via
//! [`GeneratorBuilder`]).
//!
//! # Generated artifacts
//!
//! Generation produces one `{package}.rest.rs` per proto package plus a single
//! top-level `transcoder.rest.rs` spanning all services:
//!
//! - **A service trait** per service, with one `async fn` per RPC taking the
//!   decoded request message and a `&mut Context` and returning
//!   `Result<Response, Status>` (a server-streaming RPC returns
//!   `Result<ResponseStream<Response>, Status>`).
//! - **A top-level `Transcoder`** that routes an incoming request to the right
//!   handler. It implements [`Transcode`](crate::transcoding::Transcode) (`try_transcode` /
//!   `transcode`), returning a [`TranscodeResponse`](crate::transcoding::TranscodeResponse)
//!   that serves both unary and server-streaming RPCs, so bring the trait into
//!   scope to call those methods.
//! - **An optional `tonic` bridge** per service — a blanket
//!   `impl <Trait> for T where T: {service}_server::{Service}` that forwards each
//!   RPC to a `tonic`-generated server, so one implementation serves gRPC and
//!   REST.
//!
//! The generated code covers unary and server-streaming RPCs, including
//! `additional_bindings`, `response_body`, and custom verbs; client-streaming
//! and bidirectional RPCs have no REST mapping and are rejected.
//!
//! # OpenAPI
//!
//! With the `build-openapi` feature, configure
//! [`GeneratorBuilder::emit_openapi_spec`]
//! with the API metadata; [`Generator::write`] then
//! writes a `{module}.openapi.json` beside each `{module}.rest.rs`, or
//! [`GeneratedOutput::openapi_spec`]
//! hands you the document. OpenAPI output is only available for services decoded
//! from a descriptor.
//!
//! # Examples
//!
//! Define a service by hand and generate its REST trait and transcoder:
//!
//! ```
//! use rest_over_grpc::build::{Generator, HttpMethod, HttpRule, ServiceDefinition};
//!
//! let rule = HttpRule::new(
//!     "GetShelf",
//!     HttpMethod::Get,
//!     "/v1/shelves/{shelf}"
//!         .parse()
//!         .expect("the path template is valid"),
//! );
//! let mut library = ServiceDefinition::new("Library", None);
//! library.add_method(rule, "crate::pb::GetShelfRequest", "crate::pb::Shelf", None);
//!
//! let (transcoder, generated) = Generator::new().add(library).generate();
//! assert!(
//!     generated[0]
//!         .r#trait()
//!         .to_string()
//!         .contains("pub trait Library")
//! );
//! assert!(transcoder.to_string().contains("Transcoder"));
//! ```

mod binding;
mod descriptor;
mod descriptor_error;
mod descriptor_options;
mod emit;
mod generator;
mod generator_builder;
mod generator_output;
mod http_rule;
#[cfg(feature = "build-openapi")]
mod openapi;
mod request_body;
mod response_body;
mod route;
mod service_definition;
mod service_method;

pub use binding::Binding;
pub use descriptor_error::DescriptorError;
pub use descriptor_options::DescriptorOptions;
// Internal primitive, `pub` only so the `rest_over_grpc_tests` crate can build a
// bare `Route::resolve`; not part of the documented public API.
#[doc(hidden)]
pub use emit::generate_router;
pub use generator::{Generator, compile_fds};
pub use generator_builder::GeneratorBuilder;
pub use generator_output::GeneratedOutput;
pub use http_rule::HttpRule;
#[cfg(feature = "build-openapi")]
pub use openapi::OpenApiInfo;
pub use request_body::RequestBody;
pub use response_body::ResponseBody;
pub use routerama_build::HttpMethod;
pub use service_definition::ServiceDefinition;
