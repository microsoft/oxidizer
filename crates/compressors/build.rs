// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Defines `cfg(any_format)`: true when at least one format feature is enabled.
//!
//! Several items exist only to hold the crate together when no format is compiled -- the
//! uninhabited `dispatch!` arm, and the lint suppressions for the parameters and helpers that arm
//! leaves unused. Spelling that condition inline means repeating all five format features at every
//! such site, and every future format has to be added to each copy. Naming it once here covers
//! them all.
//!
//! This is derived from the enabled features rather than declared as a feature of its own on
//! purpose. An internal feature that each format turned on would look equivalent, but Cargo lets
//! anything enable it directly, and `cargo hack --feature-powerset` does exactly that: it would
//! compile a configuration claiming a format exists while none does, which is the opposite of what
//! these sites assume. A derived cfg cannot disagree with the features it is computed from, and it
//! adds nothing to the powerset.

use cfg_aliases::cfg_aliases;

fn main() {
    println!("cargo::rerun-if-changed=build.rs");

    // Emits the matching `rustc-check-cfg` too, so `unexpected_cfgs` still catches a misspelling.
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
