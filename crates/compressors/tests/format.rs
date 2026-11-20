// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Content encoding and enabled format contracts.

use compressors::format::Format;

#[cfg(not(any(feature = "brotli", feature = "deflate", feature = "gzip", feature = "zlib", feature = "zstd")))]
#[test]
fn unknown_content_encoding_is_rejected_without_format_features() {
    assert_eq!(Format::from_content_encoding("identity"), None);
}

#[test]
fn content_encoding_tokens_round_trip() {
    for &format in Format::ALL {
        let Some(token) = format.content_encoding() else {
            continue;
        };

        assert_eq!(
            Format::from_content_encoding(token),
            Some(format),
            "{format:?} did not survive its own token"
        );
    }
}

#[cfg(all(feature = "deflate", feature = "zlib"))]
#[test]
fn http_deflate_token_means_zlib() {
    assert_eq!(Format::from_content_encoding("deflate"), Some(Format::Zlib));
    assert_eq!(Format::Deflate.content_encoding(), None);
}

#[cfg(feature = "gzip")]
#[test]
fn content_encoding_parsing_is_case_insensitive_and_trims() {
    assert_eq!(Format::from_content_encoding("GZIP"), Some(Format::Gzip));
    assert_eq!(Format::from_content_encoding("  gzip  "), Some(Format::Gzip));
    assert_eq!(Format::from_content_encoding("x-gzip"), Some(Format::Gzip));
    assert_eq!(Format::from_content_encoding("identity"), None);
    assert_eq!(Format::from_content_encoding(""), None);
}

#[cfg(feature = "brotli")]
#[test]
fn brotli_uses_the_br_token() {
    assert_eq!(Format::from_content_encoding("br"), Some(Format::Brotli));
    assert_eq!(Format::Brotli.content_encoding(), Some("br"));
}

#[test]
fn unknown_content_encoding_tokens_are_rejected() {
    assert_eq!(Format::from_content_encoding("compress"), None);
    assert_eq!(Format::from_content_encoding("identity"), None);
}

#[test]
fn format_all_lists_exactly_the_enabled_formats() {
    let expected = usize::from(cfg!(feature = "deflate"))
        + usize::from(cfg!(feature = "zlib"))
        + usize::from(cfg!(feature = "gzip"))
        + usize::from(cfg!(feature = "brotli"))
        + usize::from(cfg!(feature = "zstd"));

    assert_eq!(Format::ALL.len(), expected);
}
