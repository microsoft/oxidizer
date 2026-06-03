// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! `ALPN` protocol mapping from supported HTTP versions.

use http::Version;

const HTTP_11_ALPN: &str = "http/1.1";
const HTTP_2_ALPN: &str = "h2";

/// Maps configured HTTP versions to the advertised `ALPN` identifiers.
pub(crate) fn map_to_alpn(versions: &[Version]) -> &[&str] {
    let http1 = supports_http1(versions);
    let http2 = supports_http2(versions);
    if http2 && http1 {
        &[HTTP_2_ALPN, HTTP_11_ALPN]
    } else if http2 {
        &[HTTP_2_ALPN]
    } else if http1 {
        &[HTTP_11_ALPN]
    } else {
        &[]
    }
}

fn supports_http1(versions: &[Version]) -> bool {
    versions.contains(&Version::HTTP_11) || versions.contains(&Version::HTTP_10)
}

fn supports_http2(versions: &[Version]) -> bool {
    versions.contains(&Version::HTTP_2)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case(&[Version::HTTP_11, Version::HTTP_2], &["h2", "http/1.1"])]
    #[case(&[Version::HTTP_2], &["h2"])]
    #[case(&[Version::HTTP_11], &["http/1.1"])]
    #[case(&[Version::HTTP_10], &["http/1.1"])]
    #[case(&[], &[])]
    #[case(&[Version::HTTP_3], &[])]
    #[case(&[Version::HTTP_10, Version::HTTP_2], &["h2", "http/1.1"])]
    fn map_to_alpn(#[case] versions: &[Version], #[case] expected_str: &[&str]) {
        assert_eq!(super::map_to_alpn(versions), expected_str);
    }
}
