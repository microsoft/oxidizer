// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(not(feature = "std"), no_std)]
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![deny(unsafe_op_in_unsafe_fn)]
#![doc(hidden)]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/http_headers_simd/logo.png")]
#![doc(
    html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/http_headers_simd/favicon.ico"
)]

//! SIMD implementation details for the
//! [`http_headers`](https://docs.rs/http_headers) crate.
//!
//! **Do not depend on this crate directly.** Use `http_headers` instead.
//!
//! The default `std` feature enables runtime CPU-feature detection.
//! With default features disabled, dispatch uses compile-time target features
//! and architecture-specific fallbacks. The `benchmarking` and `test-util`
//! features expose unstable repository instrumentation only.

#[cfg(test)]
extern crate std;

mod api;
mod base64;
mod dispatch;
mod list;
mod range;
mod scalar;
mod uri;

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
mod x86;

#[cfg(target_arch = "aarch64")]
mod arm;

#[cfg(all(feature = "benchmarking", feature = "std"))]
pub mod tracking;

/// Direct reference-backend access for differential benchmarks.
#[cfg(feature = "benchmarking")]
pub mod benchmarking;

#[cfg(any(feature = "benchmarking", feature = "test-util"))]
#[doc(inline)]
pub use api::{Backend, backend, backend_for, simd_threshold};
#[doc(inline)]
pub use api::{
    all_base64_alphabet, as_simple_uri_reference, ascii_str, eq_ignore_ascii_case, find_either, find_interesting, is_field_value,
    is_simple_uri_path, is_token, is_token68, scan_byte_range_set, scan_token_list,
};
#[doc(inline)]
pub use list::{EmptyMembers, TokenListScan};
