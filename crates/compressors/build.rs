// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Defines `cfg(any_format)`: true when at least one format feature is enabled.
//!
//! Some items exist only when no format is compiled. These items include the uninhabited
//! `dispatch!` arm and its lint suppressions. An inline condition must repeat all five format
//! features at each site. This alias keeps the condition in one place.
//!
//! The build derives this alias from the enabled features. It is not a Cargo feature because any
//! dependency can enable a Cargo feature directly. `cargo hack --feature-powerset` also enables
//! features directly. A direct enablement can claim that a format exists when none exists.
//! A derived `cfg` always matches its source features and does not add to the feature powerset.

use cfg_aliases::cfg_aliases;

fn main() {
    println!("cargo::rerun-if-changed=build.rs");

    // This also emits the matching `rustc-check-cfg`. `unexpected_cfgs` still detects a misspelling.
    cfg_aliases! {
        any_format: {
            any(
                feature = "brotli",
                feature = "deflate",
                feature = "gzip",
                feature = "zlib",
                feature = "zstd"
            )
        },
    }
}
