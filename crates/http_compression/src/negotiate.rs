// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Picking a compression format from an `Accept-Encoding` header.

use compressors::format::Format;
use http::HeaderMap;
use http::header::{ACCEPT_ENCODING, CONTENT_ENCODING};

/// The quality value of one `Accept-Encoding` entry, scaled to thousandths.
///
/// RFC 9110 allows three decimal places and no more, so thousandths hold every
/// legal value exactly and compare without floating point.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Quality(u16);

impl Quality {
    const ZERO: Self = Self(0);
    const MAX: Self = Self(1000);

    fn new(value: u16) -> Option<Self> {
        (value <= Self::MAX.0).then_some(Self(value))
    }

    fn is_acceptable(self) -> bool {
        self > Self::ZERO
    }
}

/// The result of negotiating an encoding for one response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Selection {
    /// Apply this compression format.
    Format(Format),
    /// Send the representation without a content coding.
    Identity,
    /// Neither identity nor an offered format is acceptable.
    NotAcceptable,
}

/// Chooses the best representation the client will accept.
///
/// `offered` is in server preference order, which breaks ties between compression formats
/// the client rates equally. An entry the client rejects outright with `q=0` is
/// never chosen, and a format absent from the header is only chosen when the
/// header allows a wildcard.
pub(crate) fn select(headers: &HeaderMap, offered: &[Format]) -> Selection {
    if offered.is_empty() {
        return Selection::Identity;
    }

    if !headers.contains_key(ACCEPT_ENCODING) {
        return Selection::Identity;
    }

    let mut best: Option<(Format, Quality)> = None;

    for format in offered {
        let Some(token) = format.content_encoding() else {
            continue;
        };

        let Some(quality) = quality_for(headers, token, Wildcard::Allowed).filter(|quality| quality.is_acceptable()) else {
            continue;
        };

        // Strictly greater, so the first of two equally rated formats wins and
        // the server's own preference order decides.
        if best.is_none_or(|(_, best_quality)| quality > best_quality) {
            best = Some((*format, quality));
        }
    }

    // `identity` is the caller asking for no compression at all. If they rate it
    // at least as highly as anything on offer, honouring the preference means
    // sending the body as it is.
    // Only a named `identity` says so. A bare `*` means anything is acceptable,
    // not that the caller would rather have nothing applied.
    let explicit_identity = quality_for(headers, "identity", Wildcard::Ignored);
    let identity_acceptable = identity_acceptable(headers);
    let identity_preference = explicit_identity.unwrap_or(Quality::ZERO);

    if let Some((format, _)) = best.filter(|(_, quality)| *quality > identity_preference) {
        Selection::Format(format)
    } else if identity_acceptable {
        Selection::Identity
    } else {
        Selection::NotAcceptable
    }
}

/// Returns whether the client permits a response without a content coding.
pub(crate) fn identity_acceptable(headers: &HeaderMap) -> bool {
    if !headers.contains_key(ACCEPT_ENCODING) {
        return true;
    }

    let explicit_identity = quality_for(headers, "identity", Wildcard::Ignored);
    let rejected_by_wildcard = explicit_identity.is_none() && quality_for(headers, "\0", Wildcard::Allowed) == Some(Quality::ZERO);

    explicit_identity.is_none_or(Quality::is_acceptable) && !rejected_by_wildcard
}

/// Returns whether the client permits the response's existing content coding.
pub(crate) fn content_encoding_acceptable(request_headers: &HeaderMap, response_headers: &HeaderMap) -> bool {
    if !request_headers.contains_key(ACCEPT_ENCODING) {
        return true;
    }

    let mut found = false;
    for value in response_headers.get_all(CONTENT_ENCODING) {
        let Ok(value) = value.to_str() else {
            return false;
        };

        for token in value.split(',').map(str::trim) {
            if token.is_empty() {
                continue;
            }
            found = true;

            let acceptable = if token.eq_ignore_ascii_case("identity") {
                identity_acceptable(request_headers)
            } else {
                quality_for(request_headers, token, Wildcard::Allowed).is_some_and(Quality::is_acceptable)
            };
            if !acceptable {
                return false;
            }
        }
    }

    found
}

/// Whether `*` may stand in for a format the header does not name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Wildcard {
    Allowed,
    Ignored,
}

/// The quality the header assigns to `token`, including zero, or `None` if it is absent.
///
/// A list header may be split across several lines, so every one is read in
/// order, as though they had been joined with commas.
fn quality_for(headers: &HeaderMap, token: &str, wildcard_use: Wildcard) -> Option<Quality> {
    let mut wildcard = None;

    for value in headers.get_all(ACCEPT_ENCODING) {
        let Ok(value) = value.to_str() else {
            continue;
        };

        for entry in value.split(',') {
            let mut parts = entry.split(';');
            let name = parts.next()?.trim();

            let quality = parts.next().map_or(Some(Quality::MAX), |weight| {
                let weight = weight.trim();
                weight
                    .strip_prefix("q=")
                    .or_else(|| weight.strip_prefix("Q="))
                    .and_then(parse_quality)
            });
            let Some(quality) = quality else {
                continue;
            };
            if parts.next().is_some() {
                continue;
            }

            if name.eq_ignore_ascii_case(token) {
                // An explicit entry always beats the wildcard, even to reject.
                return Some(quality);
            }

            if name == "*" && wildcard_use == Wildcard::Allowed {
                wildcard = Some(quality);
            }
        }
    }

    wildcard
}

/// Parses a `q=` value into thousandths, rejecting anything malformed.
fn parse_quality(value: &str) -> Option<Quality> {
    let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));

    let whole = match whole {
        "0" => 0,
        "1" => Quality::MAX.0,
        _ => return None,
    };
    if fraction.len() > 3 || !fraction.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }

    // Pad so "5" and "500" both mean 0.5.
    let mut thousandths = 0;
    for index in 0..3 {
        thousandths = thousandths * 10 + u16::from(fraction.as_bytes().get(index).map_or(0, |b| b - b'0'));
    }

    let quality = whole + thousandths;

    Quality::new(quality)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use http::HeaderValue;

    use super::*;

    fn headers(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT_ENCODING, HeaderValue::from_str(value).unwrap());
        headers
    }

    fn split_headers(values: &[&str]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for value in values {
            headers.append(ACCEPT_ENCODING, HeaderValue::from_str(value).unwrap());
        }
        headers
    }

    const OFFERED: &[Format] = &[Format::Zstd, Format::Brotli, Format::Gzip];

    #[test]
    fn nothing_is_selected_without_a_header() {
        assert_eq!(select(&HeaderMap::new(), OFFERED), Selection::Identity);
    }

    #[test]
    fn nothing_is_selected_when_nothing_is_offered() {
        assert_eq!(select(&headers("gzip"), &[]), Selection::Identity);
    }

    #[test]
    fn a_format_without_an_http_token_is_not_selected() {
        assert_eq!(select(&headers("*"), &[Format::Deflate]), Selection::Identity);
    }

    #[test]
    fn the_only_acceptable_format_is_selected() {
        assert_eq!(select(&headers("gzip"), OFFERED), Selection::Format(Format::Gzip));
    }

    #[test]
    fn the_highest_quality_wins() {
        assert_eq!(select(&headers("gzip;q=0.9, br;q=0.2"), OFFERED), Selection::Format(Format::Gzip));
        assert_eq!(select(&headers("gzip;q=0.2, br;q=0.9"), OFFERED), Selection::Format(Format::Brotli));
    }

    #[test]
    fn a_tie_is_broken_by_the_offered_order() {
        // Zstd is offered first, so it wins an equal rating.
        assert_eq!(select(&headers("gzip, br, zstd"), OFFERED), Selection::Format(Format::Zstd));
    }

    #[test]
    fn a_preference_for_no_compression_is_respected() {
        // Rating `identity` above the alternatives is how a caller says it
        // would rather have the body uncompressed.
        assert_eq!(select(&headers("identity, gzip;q=0.5"), OFFERED), Selection::Identity);
        assert_eq!(select(&headers("identity;q=0.5, gzip;q=0.5"), OFFERED), Selection::Identity);
        assert_eq!(select(&headers("identity;q=0.5, gzip"), OFFERED), Selection::Format(Format::Gzip));
        assert_eq!(
            select(&headers("identity;q=0, gzip;q=0.1"), OFFERED),
            Selection::Format(Format::Gzip)
        );
    }

    #[test]
    fn zero_quality_is_not_acceptable() {
        assert!(!Quality::ZERO.is_acceptable());
        assert!(Quality::MAX.is_acceptable());
    }

    #[test]
    fn an_explicitly_rejected_format_is_never_selected() {
        assert_eq!(select(&headers("gzip;q=0"), OFFERED), Selection::Identity);
        assert_eq!(select(&headers("gzip;q=0, br"), OFFERED), Selection::Format(Format::Brotli));
    }

    #[test]
    fn a_wildcard_covers_formats_the_header_does_not_name() {
        assert_eq!(select(&headers("*"), OFFERED), Selection::Format(Format::Zstd));
        // An explicit entry overrides the wildcard, even to reject.
        assert_eq!(select(&headers("*, zstd;q=0"), OFFERED), Selection::Format(Format::Brotli));
    }

    #[test]
    fn a_rejecting_wildcard_leaves_only_named_formats() {
        assert_eq!(select(&headers("*;q=0"), OFFERED), Selection::NotAcceptable);
        assert_eq!(select(&headers("*;q=0, gzip"), OFFERED), Selection::Format(Format::Gzip));
    }

    #[test]
    fn quality_is_parsed_to_three_decimal_places() {
        assert_eq!(parse_quality("1"), Some(Quality(1000)));
        assert_eq!(parse_quality("1.0"), Some(Quality(1000)));
        assert_eq!(parse_quality("0.5"), Some(Quality(500)));
        assert_eq!(parse_quality("0.05"), Some(Quality(50)));
        assert_eq!(parse_quality("0.005"), Some(Quality(5)));
        assert_eq!(parse_quality("0"), Some(Quality(0)));
        assert_eq!(parse_quality("0."), Some(Quality(0)));
        assert_eq!(parse_quality("1."), Some(Quality(1000)));
    }

    #[test]
    fn malformed_quality_is_rejected() {
        assert_eq!(parse_quality("2"), None, "above the maximum");
        assert_eq!(parse_quality("1.001"), None, "above the maximum");
        assert_eq!(parse_quality("0.0001"), None, "too many decimals");
        assert_eq!(parse_quality("abc"), None);
        assert_eq!(parse_quality("0.x"), None);
        assert_eq!(parse_quality("+1"), None);
        assert_eq!(parse_quality("01"), None);
        assert_eq!(parse_quality("00.5"), None);
    }

    #[test]
    fn malformed_members_do_not_discard_valid_alternatives() {
        assert_eq!(select(&headers("gzip;q=nonsense"), OFFERED), Selection::Identity);
        for invalid in [
            "gzip;q=nonsense",
            "gzip;q=2",
            "gzip;q=0.1234",
            "gzip;q=+1",
            "gzip;q=01",
            "gzip;q=00.5",
            "gzip; q = 1",
            "gzip;quality=1",
            "gzip;q=0;q=1",
            "gzip;q=1;extra",
        ] {
            for value in [format!("{invalid}, br"), format!("br, {invalid}")] {
                assert_eq!(select(&headers(&value), OFFERED), Selection::Format(Format::Brotli), "{value}");
            }
        }
        assert_eq!(
            select(&split_headers(&["gzip;q=nonsense", "br"]), OFFERED),
            Selection::Format(Format::Brotli)
        );
        assert_eq!(select(&headers("gzip;q=nonsense, gzip"), OFFERED), Selection::Format(Format::Gzip));
    }

    #[test]
    fn ignoring_malformed_members_preserves_rejections_and_identity_preferences() {
        assert_eq!(select(&headers("br;q=nonsense, gzip;q=0"), &[Format::Gzip]), Selection::Identity);
        assert_eq!(select(&headers("br;q=nonsense, *;q=0"), &[Format::Gzip]), Selection::NotAcceptable);
        assert_eq!(select(&headers("gzip;q=0, br;q=nonsense, *"), &[Format::Gzip]), Selection::Identity);
        assert_eq!(
            select(&headers("gzip;q=nonsense, br;q=0.5, identity"), OFFERED),
            Selection::Identity
        );
        assert_eq!(
            select(&headers("gzip;q=nonsense, br;q=0.5, identity;q=0"), OFFERED),
            Selection::Format(Format::Brotli)
        );
        assert_eq!(
            select(&headers("gzip;q=nonsense, *;q=0.5"), OFFERED),
            Selection::Format(Format::Zstd)
        );
        assert_eq!(select(&headers("*;q=nonsense, gzip"), OFFERED), Selection::Format(Format::Gzip));
    }

    #[test]
    fn an_unreadable_header_line_does_not_discard_readable_lines() {
        let mut headers = headers("br");
        headers.append(ACCEPT_ENCODING, HeaderValue::from_bytes(b"\xff").unwrap());
        assert_eq!(select(&headers, OFFERED), Selection::Format(Format::Brotli));

        headers.insert(ACCEPT_ENCODING, HeaderValue::from_bytes(b"\xff").unwrap());
        headers.append(ACCEPT_ENCODING, HeaderValue::from_static("gzip"));
        assert_eq!(select(&headers, OFFERED), Selection::Format(Format::Gzip));
    }

    #[test]
    fn a_header_split_over_several_lines_is_read_as_one_list() {
        // RFC 9110 lets a list header be sent as repeated lines.
        assert_eq!(
            select(&split_headers(&["gzip;q=0.2", "br;q=0.9"]), OFFERED),
            Selection::Format(Format::Brotli)
        );
        assert_eq!(
            select(&split_headers(&["gzip", "zstd;q=0"]), OFFERED),
            Selection::Format(Format::Gzip)
        );
        assert_eq!(select(&split_headers(&["identity", "*;q=0"]), OFFERED), Selection::Identity);
    }

    #[test]
    fn casing_and_whitespace_do_not_matter() {
        assert_eq!(select(&headers("  GZIP ;  q=0.9  "), OFFERED), Selection::Format(Format::Gzip));
    }

    #[test]
    fn rejecting_identity_and_every_offered_format_is_not_acceptable() {
        assert_eq!(
            select(&headers("gzip;q=0, identity;q=0"), &[Format::Gzip]),
            Selection::NotAcceptable
        );
    }

    #[test]
    fn identity_is_acceptable_without_an_accept_encoding_header() {
        assert!(identity_acceptable(&HeaderMap::new()));
    }

    #[test]
    fn existing_content_encoding_acceptance_handles_header_edge_cases() {
        let response = |value: HeaderValue| {
            let mut headers = HeaderMap::new();
            headers.insert(CONTENT_ENCODING, value);
            headers
        };

        assert!(content_encoding_acceptable(
            &HeaderMap::new(),
            &response(HeaderValue::from_static("br"))
        ));
        assert!(!content_encoding_acceptable(
            &headers("gzip"),
            &response(HeaderValue::from_bytes(b"\xff").unwrap())
        ));
        assert!(!content_encoding_acceptable(
            &headers("gzip"),
            &response(HeaderValue::from_static(",,"))
        ));
        assert!(content_encoding_acceptable(
            &headers("gzip"),
            &response(HeaderValue::from_static("gzip, "))
        ));
        assert!(!content_encoding_acceptable(
            &headers("identity;q=0"),
            &response(HeaderValue::from_static("identity"))
        ));
        assert!(content_encoding_acceptable(
            &headers("gzip, br"),
            &response(HeaderValue::from_static("gzip, br"))
        ));
    }
}
