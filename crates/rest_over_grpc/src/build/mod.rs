// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! REST-layer code generation for services behind the `build` feature.
//!
//! Use [`compile_fds`] for the common `build.rs` flow from a descriptor set, or
//! build [`ServiceDefinition`] values by hand and render them with a
//! [`Generator`]. Generated output includes one `{package}.rest.rs` per module,
//! an optional `{module}.openapi.json`, and a top-level `transcoder.rest.rs`.
//!
//! The generated REST code handles unary and server-streaming RPCs, additional
//! bindings, custom verbs, and optional `tonic` bridging. Client-streaming and
//! bidirectional RPCs are rejected because they have no REST mapping.
//!
//! This module consumes descriptors but does not generate protobuf message
//! types or their serde implementations. Generate the messages separately and
//! provide proto3-JSON-compatible serde implementations, normally with
//! `pbjson-build`. [`compile_fds`] emits the `tonic` bridge by default. The
//! generated `__rest_over_grpc_bridge_{Service}::{Service}RestBridge`
//! requires `with_guard(service, guard)` or an
//! explicit `externally_authenticated(service)` acknowledgment; tonic
//! interceptors do not run on REST requests. Call
//! [`GeneratorBuilder::emit_tonic_bridge`] with `false` when implementing the
//! generated REST trait directly or using another gRPC stack.
//!
//! ```ignore
//! use rest_over_grpc::build::{DescriptorOptions, Generator, ServiceDefinition};
//!
//! let descriptor_set = std::fs::read("target/file_descriptor_set.bin")?;
//! Generator::new()
//!     .add_all(ServiceDefinition::from_fds(&descriptor_set, &DescriptorOptions::new())?)
//!     .write(std::env::var("OUT_DIR")?)?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
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

#[doc(inline)]
pub use binding::Binding;
#[doc(inline)]
pub use descriptor_error::DescriptorError;
#[doc(inline)]
pub use descriptor_options::DescriptorOptions;
// Internal primitive exposed only for the workspace's routing tests and benchmarks.
#[cfg(any(test, feature = "private-test-util"))]
#[doc(hidden)]
pub use emit::generate_router;
#[doc(inline)]
pub use generator::{Generator, compile_fds};
#[doc(inline)]
pub use generator_builder::GeneratorBuilder;
#[doc(inline)]
pub use generator_output::GeneratedOutput;
#[doc(inline)]
pub use http_rule::HttpRule;
#[cfg(feature = "build-openapi")]
#[cfg_attr(docsrs, doc(cfg(feature = "build-openapi")))]
#[doc(inline)]
pub use openapi::OpenApiInfo;
#[doc(inline)]
pub use request_body::RequestBody;
#[doc(inline)]
pub use response_body::ResponseBody;
#[doc(inline)]
pub use service_definition::ServiceDefinition;
