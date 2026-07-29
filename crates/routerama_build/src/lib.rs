// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![no_std]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/routerama_build/logo.png")]
#![doc(
    html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/routerama_build/favicon.ico"
)]

//! Static resolver code generation for [`routerama`](https://docs.rs/routerama).
//!
//! [`Route`] stores validated path templates and their generated variant names.
//! [`Generator`](https://docs.rs/routerama_build/latest/routerama_build/?search=Generator)
//! collects routes and emits a resolver as a
//! [`proc_macro2::TokenStream`](https://docs.rs/proc-macro2/latest/proc_macro2/struct.TokenStream.html).
//! This API is intended for build scripts and
//! procedural-macro implementations; applications normally use
//! `routerama::resolver` instead.
//!
//! Disable the default `codegen` feature when only the hidden, framework-neutral
//! routing trie is required at run time.
//!
//! # Examples
//!
//! ```
//! # #[cfg(feature = "codegen")]
//! use http_path_template::{Grammar, PathTemplate};
//! # #[cfg(feature = "codegen")]
//! use routerama_build::{Generator, Route};
//!
//! # #[cfg(feature = "codegen")]
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let mut generator = Generator::new("Route", true);
//! generator.add(Route::new(
//!     "GetBook",
//!     "GET",
//!     PathTemplate::parse("/books/{book}", Grammar::default())?,
//! ));
//!
//! let generated = generator.generate().to_string();
//! assert!(generated.contains("GetBook"));
//! # Ok(())
//! # }
//! # #[cfg(not(feature = "codegen"))]
//! # fn main() {}
//! ```

extern crate alloc;
#[cfg(any(test, feature = "codegen"))]
extern crate std;

#[cfg(feature = "codegen")]
mod codegen;
#[cfg(feature = "codegen")]
mod generator;
#[cfg(feature = "codegen")]
#[doc(hidden)]
pub mod macro_impl;
mod route;
#[doc(hidden)]
pub mod trie;

#[cfg(feature = "codegen")]
pub use generator::Generator;
pub use route::Route;
#[doc(hidden)]
pub use route::is_http_token;
#[doc(hidden)]
pub use trie::route_field_name;
