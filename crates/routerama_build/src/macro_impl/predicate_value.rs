// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::is_http_token;

#[cfg(any(feature = "resolve", feature = "route"))]
pub(super) fn is_concrete_media_type(value: &str) -> bool {
    let Some((top_level, subtype)) = value.split_once('/') else {
        return false;
    };
    !top_level.contains('/')
        && !subtype.contains('/')
        && top_level != "*"
        && subtype != "*"
        && is_http_token(top_level)
        && is_http_token(subtype)
}

// This compile-time validation mirrors `routerama::route::predicate`. The
// macro implementation cannot depend on the runtime crate without creating a
// package cycle.
#[cfg(any(feature = "resolve", feature = "route"))]
pub(super) fn is_http_authority(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.is_empty()
        || bytes
            .iter()
            .any(|byte| !byte.is_ascii() || byte.is_ascii_whitespace() || matches!(byte, b'/' | b'?' | b'#' | b'@' | b'\\'))
    {
        return false;
    }

    if bytes[0] == b'[' {
        let Some(end) = bytes.iter().position(|byte| *byte == b']') else {
            return false;
        };
        if end == 1 || !valid_ip_literal(&value[1..end]) {
            return false;
        }
        return valid_authority_port(&value[end + 1..]);
    }

    if bytes.contains(&b'[') || bytes.contains(&b']') {
        return false;
    }
    let (host, port) = match value.split_once(':') {
        Some((host, port)) if !port.contains(':') => (host, Some(port)),
        Some(_) => return false,
        None => (value, None),
    };
    !host.is_empty() && valid_reg_name(host) && port.is_none_or(valid_port)
}

#[cfg(any(feature = "resolve", feature = "route"))]
fn valid_authority_port(suffix: &str) -> bool {
    suffix.is_empty() || suffix.strip_prefix(':').is_some_and(valid_port)
}

#[cfg(any(feature = "resolve", feature = "route"))]
fn valid_port(port: &str) -> bool {
    if port.is_empty() || !port.bytes().all(|byte| byte.is_ascii_digit()) {
        return false;
    }
    port.bytes()
        .try_fold(0_u16, |value, byte| {
            value.checked_mul(10).and_then(|value| value.checked_add(u16::from(byte - b'0')))
        })
        .is_some()
}

#[cfg(any(feature = "resolve", feature = "route"))]
fn valid_reg_name(value: &str) -> bool {
    valid_percent_encoded_sequence(value.as_bytes(), |byte| {
        byte.is_ascii_alphanumeric()
            || matches!(
                byte,
                b'-' | b'.' | b'_' | b'~' | b'!' | b'$' | b'&' | b'\'' | b'(' | b')' | b'*' | b'+' | b',' | b';' | b'='
            )
    })
}

#[cfg(any(feature = "resolve", feature = "route"))]
fn valid_ip_literal(value: &str) -> bool {
    if let Some(version) = value.strip_prefix('v').or_else(|| value.strip_prefix('V')) {
        let Some((version, address)) = version.split_once('.') else {
            return false;
        };
        return !version.is_empty()
            && version.bytes().all(|byte| byte.is_ascii_hexdigit())
            && !address.is_empty()
            && address.bytes().all(|byte| {
                matches!(
                    byte,
                    b'0'..=b'9' | b'a'..=b'z' | b'A'..=b'Z'
                        | b'-' | b'.' | b'_' | b'~' | b'!' | b'$' | b'&' | b'\'' | b'(' | b')' | b'*' | b'+' | b',' | b';' | b'=' | b':'
                )
            });
    }

    let (address, zone) = value
        .split_once("%25")
        .map_or((value, None), |(address, zone)| (address, Some(zone)));
    address.parse::<core::net::Ipv6Addr>().is_ok()
        && zone.is_none_or(|zone| {
            !zone.is_empty()
                && valid_percent_encoded_sequence(zone.as_bytes(), |byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~')
                })
        })
}

#[cfg(any(feature = "resolve", feature = "route"))]
fn valid_percent_encoded_sequence(bytes: &[u8], ordinary: impl Fn(u8) -> bool) -> bool {
    let mut index = 0;
    while index < bytes.len() {
        if ordinary(bytes[index]) {
            index += 1;
        } else if bytes[index] == b'%'
            && bytes
                .get(index + 1..index + 3)
                .is_some_and(|digits| digits.iter().all(u8::is_ascii_hexdigit))
        {
            index += 3;
        } else {
            return false;
        }
    }
    true
}

#[cfg(all(test, any(feature = "resolve", feature = "route")))]
mod tests {
    use super::*;

    #[test]
    fn concrete_media_type_accepts_and_rejects() {
        assert!(is_concrete_media_type("application/json"));
        assert!(!is_concrete_media_type("no-slash"));
        assert!(!is_concrete_media_type("*/json"));
        assert!(!is_concrete_media_type("application/*"));
        assert!(!is_concrete_media_type("app/lication/json"));
    }

    #[test]
    fn http_authority_rejects_unterminated_ip_literal_bracket() {
        // No closing `]` at all: hits the `let Some(end) = ... else` branch.
        assert!(!is_http_authority("[2001:db8::1"));
    }

    #[test]
    fn http_authority_rejects_empty_or_invalid_ip_literal() {
        // `end == 1` means an empty bracketed literal (`[]`).
        assert!(!is_http_authority("[]"));
        // A non-hex, non-IPv6 literal fails `valid_ip_literal`.
        assert!(!is_http_authority("[not-ipv6]"));
    }

    #[test]
    fn http_authority_accepts_valid_ip_literal_with_port() {
        assert!(is_http_authority("[2001:db8::1]"));
        assert!(is_http_authority("[2001:db8::1]:8443"));
        assert!(!is_http_authority("[2001:db8::1]:not-a-port"));
    }

    #[test]
    fn http_authority_rejects_stray_brackets_without_leading_bracket() {
        assert!(!is_http_authority("abc]"));
        assert!(!is_http_authority("abc["));
    }

    #[test]
    fn http_authority_rejects_multiple_colons_without_brackets() {
        // More than one `:` outside of an IP literal hits the `Some(_) =>
        // return false` arm.
        assert!(!is_http_authority("api.example:80:90"));
    }

    #[test]
    fn http_authority_accepts_host_without_port() {
        assert!(is_http_authority("api.example"));
        assert!(is_http_authority("api.example:443"));
        assert!(!is_http_authority("api.example:65536"));
        assert!(!is_http_authority(""));
    }

    #[test]
    fn ip_literal_version_prefix_accepts_and_rejects() {
        // The `vN.address` future-version form.
        assert!(is_http_authority("[v1.something]"));
        assert!(is_http_authority("[V1.something]"));
        // Empty version or address is rejected.
        assert!(!is_http_authority("[v.something]"));
        assert!(!is_http_authority("[v1.]"));
        // No `.` separator at all.
        assert!(!is_http_authority("[vsomething]"));
        // A non-hex version digit.
        assert!(!is_http_authority("[vz.something]"));
        // Whitespace anywhere in the authority is rejected by
        // `is_http_authority`'s own ASCII-whitespace check before ever
        // reaching `valid_ip_literal`.
        assert!(!is_http_authority("[v1.some thing]"));
        // A disallowed, non-whitespace byte in the address part: rejected by
        // `valid_ip_literal`'s own `matches!` (its implicit `_ => false` arm).
        assert!(!is_http_authority("[v1.a%b]"));
        // An address containing only punctuation (non-alphanumeric) bytes:
        // forces the `matches!` alternative of the `||` to actually run.
        assert!(is_http_authority("[v1.a-b.c_d~e!f$g&h'i(j)k*l+m,n;o=p:q]"));
        // An address ending on the very last `matches!` alternative (`:`),
        // to exercise every arm of that pattern.
        assert!(is_http_authority("[v1.:]"));
    }

    #[test]
    fn ip_literal_zone_id_accepts_and_rejects() {
        assert!(is_http_authority("[fe80::1%25eth0]"));
        // An empty zone id is rejected.
        assert!(!is_http_authority("[fe80::1%25]"));
        // A disallowed byte in the zone id.
        assert!(!is_http_authority("[fe80::1%25eth/0]"));
        // Not a valid IPv6 address at all.
        assert!(!is_http_authority("[not:an:address]"));
    }

    #[test]
    fn percent_encoded_sequence_accepts_and_rejects() {
        assert!(valid_reg_name("a%41b"));
        // An incomplete percent-escape is rejected.
        assert!(!valid_reg_name("a%4"));
        assert!(!valid_reg_name("a%zz"));
    }
}
