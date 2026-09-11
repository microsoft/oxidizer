// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Picking a compression format from an `Accept-Encoding` header.

use compressors::format::Format;
use http::HeaderMap;
use http::header::ACCEPT_ENCODING;

/// The quality value of one `Accept-Encoding` entry, scaled to thousandths.
///
/// RFC 9110 allows three decimal places and no more, so thousandths hold every
/// legal value exactly and compare without floating point.
type Quality = u16;

const MAX_QUALITY: Quality = 1000;

/// Chooses the best of `offered` that the client will accept, if any.
///
/// `offered` is in server preference order, which breaks ties between compression formats
/// the client rates equally. An entry the client rejects outright with `q=0` is
/// never chosen, and a format absent from the header is only chosen when the
/// header allows a wildcard.
pub(crate) fn select(headers: &HeaderMap, offered: &[Format]) -> Option<Format> {
    if offered.is_empty() {
        return None;
    }

    headers.get(ACCEPT_ENCODING)?;

    let mut best: Option<(Format, Quality)> = None;

    for format in offered {
        let Some(token) = format.content_encoding() else {
            continue;
        };

        let Some(quality) = quality_for(headers, token, Wildcard::Allowed) else {
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
    let identity = quality_for(headers, "identity", Wildcard::Ignored).unwrap_or(0);

    best.filter(|(_, quality)| *quality > identity).map(|(format, _)| format)
}

/// Whether `*` may stand in for a format the header does not name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Wildcard {
    Allowed,
    Ignored,
}

/// The quality the header assigns to `token`, or `None` if it is not acceptable.
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

            let quality = parts.next().map_or(Some(MAX_QUALITY), |weight| {
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
                return (quality > 0).then_some(quality);
            }

            if name == "*" && wildcard_use == Wildcard::Allowed {
                wildcard = Some(quality);
            }
        }
    }

    wildcard.filter(|quality| *quality > 0)
}

/// Parses a `q=` value into thousandths, rejecting anything malformed.
fn parse_quality(value: &str) -> Option<Quality> {
    let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));

    let whole = match whole {
        "0" => 0,
        "1" => MAX_QUALITY,
        _ => return None,
    };
    if fraction.len() > 3 || !fraction.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }

    // Pad so "5" and "500" both mean 0.5.
    let mut thousandths = 0;
    for index in 0..3 {
        thousandths = thousandths * 10 + Quality::from(fraction.as_bytes().get(index).map_or(0, |b| b - b'0'));
    }

    let quality = whole + thousandths;

    (quality <= MAX_QUALITY).then_some(quality)
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
        assert_eq!(select(&HeaderMap::new(), OFFERED), None);
    }

    #[test]
    fn nothing_is_selected_when_nothing_is_offered() {
        assert_eq!(select(&headers("gzip"), &[]), None);
    }

    #[test]
    fn the_only_acceptable_format_is_selected() {
        assert_eq!(select(&headers("gzip"), OFFERED), Some(Format::Gzip));
    }

    #[test]
    fn the_highest_quality_wins() {
        assert_eq!(select(&headers("gzip;q=0.9, br;q=0.2"), OFFERED), Some(Format::Gzip));
        assert_eq!(select(&headers("gzip;q=0.2, br;q=0.9"), OFFERED), Some(Format::Brotli));
    }

    #[test]
    fn a_tie_is_broken_by_the_offered_order() {
        // Zstd is offered first, so it wins an equal rating.
        assert_eq!(select(&headers("gzip, br, zstd"), OFFERED), Some(Format::Zstd));
    }

    #[test]
    fn a_preference_for_no_compression_is_respected() {
        // Rating `identity` above the alternatives is how a caller says it
        // would rather have the body uncompressed.
        assert_eq!(select(&headers("identity, gzip;q=0.5"), OFFERED), None);
        assert_eq!(select(&headers("identity;q=0.5, gzip"), OFFERED), Some(Format::Gzip));
        assert_eq!(select(&headers("identity;q=0, gzip;q=0.1"), OFFERED), Some(Format::Gzip));
    }

    #[test]
    fn an_explicitly_rejected_format_is_never_selected() {
        assert_eq!(select(&headers("gzip;q=0"), OFFERED), None);
        assert_eq!(select(&headers("gzip;q=0, br"), OFFERED), Some(Format::Brotli));
    }

    #[test]
    fn a_wildcard_covers_formats_the_header_does_not_name() {
        assert_eq!(select(&headers("*"), OFFERED), Some(Format::Zstd));
        // An explicit entry overrides the wildcard, even to reject.
        assert_eq!(select(&headers("*, zstd;q=0"), OFFERED), Some(Format::Brotli));
    }

    #[test]
    fn a_rejecting_wildcard_leaves_only_named_formats() {
        assert_eq!(select(&headers("*;q=0"), OFFERED), None);
        assert_eq!(select(&headers("*;q=0, gzip"), OFFERED), Some(Format::Gzip));
    }

    #[test]
    fn quality_is_parsed_to_three_decimal_places() {
        assert_eq!(parse_quality("1"), Some(1000));
        assert_eq!(parse_quality("1.0"), Some(1000));
        assert_eq!(parse_quality("0.5"), Some(500));
        assert_eq!(parse_quality("0.05"), Some(50));
        assert_eq!(parse_quality("0.005"), Some(5));
        assert_eq!(parse_quality("0"), Some(0));
        assert_eq!(parse_quality("0."), Some(0));
        assert_eq!(parse_quality("1."), Some(1000));
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
        assert_eq!(select(&headers("gzip;q=nonsense"), OFFERED), None);
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
                assert_eq!(select(&headers(&value), OFFERED), Some(Format::Brotli), "{value}");
            }
        }
        assert_eq!(select(&split_headers(&["gzip;q=nonsense", "br"]), OFFERED), Some(Format::Brotli));
        assert_eq!(select(&headers("gzip;q=nonsense, gzip"), OFFERED), Some(Format::Gzip));
    }

    #[test]
    fn ignoring_malformed_members_preserves_rejections_and_identity_preferences() {
        for value in ["br;q=nonsense, gzip;q=0", "br;q=nonsense, *;q=0", "gzip;q=0, br;q=nonsense, *"] {
            assert_eq!(select(&headers(value), &[Format::Gzip]), None, "{value}");
        }
        assert_eq!(select(&headers("gzip;q=nonsense, br;q=0.5, identity"), OFFERED), None);
        assert_eq!(
            select(&headers("gzip;q=nonsense, br;q=0.5, identity;q=0"), OFFERED),
            Some(Format::Brotli)
        );
        assert_eq!(select(&headers("gzip;q=nonsense, *;q=0.5"), OFFERED), Some(Format::Zstd));
        assert_eq!(select(&headers("*;q=nonsense, gzip"), OFFERED), Some(Format::Gzip));
    }

    #[test]
    fn an_unreadable_header_line_does_not_discard_readable_lines() {
        let mut headers = headers("br");
        headers.append(ACCEPT_ENCODING, HeaderValue::from_bytes(b"\xff").unwrap());
        assert_eq!(select(&headers, OFFERED), Some(Format::Brotli));

        headers.insert(ACCEPT_ENCODING, HeaderValue::from_bytes(b"\xff").unwrap());
        headers.append(ACCEPT_ENCODING, HeaderValue::from_static("gzip"));
        assert_eq!(select(&headers, OFFERED), Some(Format::Gzip));
    }

    #[test]
    fn a_header_split_over_several_lines_is_read_as_one_list() {
        // RFC 9110 lets a list header be sent as repeated lines.
        assert_eq!(select(&split_headers(&["gzip;q=0.2", "br;q=0.9"]), OFFERED), Some(Format::Brotli));
        assert_eq!(select(&split_headers(&["gzip", "zstd;q=0"]), OFFERED), Some(Format::Gzip));
        assert_eq!(select(&split_headers(&["identity", "*;q=0"]), OFFERED), None);
    }

    #[test]
    fn casing_and_whitespace_do_not_matter() {
        assert_eq!(select(&headers("  GZIP ;  q=0.9  "), OFFERED), Some(Format::Gzip));
    }
}
