// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Vector conversions and compatibility with the `bytes` crate.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

mod support;

mod conversions {
    #[cfg(feature = "bytes-compat")]
    mod buf;
    #[cfg(feature = "bytes-compat")]
    mod from_bytes;
    mod from_vec;
    #[cfg(feature = "bytes-compat")]
    mod to_bytes;
    #[cfg(feature = "bytes-compat")]
    mod view;
}
