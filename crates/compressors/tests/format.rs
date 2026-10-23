// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public format contract in a build with no compression backends enabled.

#![cfg(not(any(feature = "brotli", feature = "deflate", feature = "gzip", feature = "zlib", feature = "zstd")))]

use compressors::format::Format;

#[test]
fn unknown_content_encoding_is_rejected_without_format_features() {
    assert_eq!(Format::from_content_encoding("identity"), None);
}
