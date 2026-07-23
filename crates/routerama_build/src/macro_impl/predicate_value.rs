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
                byte.is_ascii_alphanumeric()
                    || matches!(
                        byte,
                        b'-' | b'.' | b'_' | b'~' | b'!' | b'$' | b'&' | b'\'' | b'(' | b')' | b'*' | b'+' | b',' | b';' | b'=' | b':'
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
