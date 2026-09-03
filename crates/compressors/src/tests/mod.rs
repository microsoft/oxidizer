// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Behaviour tests that drive the crate the way its own consumers do.
//!
//! These were integration tests until the push/pull mechanics moved onto a crate-private trait,
//! which a separate test crate cannot name. They live here so that contract can be driven by hand
//! without any of it reaching the public API.

#[cfg(any(
    test,
    feature = "brotli",
    feature = "deflate",
    feature = "gzip",
    feature = "zlib",
    feature = "zstd"
))]
mod format_contract;

#[cfg(any(test, feature = "gzip"))]
mod round_trip;
