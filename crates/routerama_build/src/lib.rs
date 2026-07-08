// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/routerama_build/logo.png")]
#![doc(
    html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/routerama_build/favicon.ico"
)]

//! Build-time code generator for an efficient static HTTP router.
//!
//! This crate is an implementation detail of the [`routerama`](https://docs.rs/routerama) crate. Please
//! see that crate for documentation.

#[cfg(feature = "codegen")]
mod codegen;
#[cfg(feature = "codegen")]
mod generator;
#[cfg(feature = "codegen")]
mod generator_builder;
mod http_method;
mod route_rule;
#[doc(hidden)]
pub mod trie;

// The public API is documented on the `routerama` facade (which re-exports it
// at its crate root under the `build` feature), not here, so these are hidden
// from this crate's own docs.
#[cfg(feature = "codegen")]
#[doc(hidden)]
pub use generator::Generator;
#[cfg(feature = "codegen")]
#[doc(hidden)]
pub use generator_builder::GeneratorBuilder;
#[doc(hidden)]
pub use http_method::HttpMethod;
#[doc(hidden)]
pub use route_rule::RouteRule;
#[doc(hidden)]
pub use trie::route_field_name;
