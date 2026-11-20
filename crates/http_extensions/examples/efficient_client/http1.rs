// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Allocation-free HTTP/1.1 parsing and formatting helpers built on the `http` crate.

use bytes::Bytes;
use bytesbuf::BytesView;
use http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use http_extensions::HttpError;
use recoverable::RecoveryInfo;

use crate::X_CORRELATION_ID;

pub(crate) fn malformed(error: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> HttpError {
    HttpError::other(error, RecoveryInfo::never(), "malformed_message")
}

pub(crate) fn shared_value(bytes: Bytes) -> Result<HeaderValue, HttpError> {
    HeaderValue::from_maybe_shared(bytes).map_err(malformed)
}

/// Tracks progress towards the `\r\n\r\n` sequence that terminates an HTTP/1.1 head, so each
/// received byte is inspected exactly once no matter how the data is chunked.
#[derive(Debug, Default)]
pub(crate) struct HeadTerminator {
    matched: usize,
}

impl HeadTerminator {
    const PATTERN: &[u8] = b"\r\n\r\n";

    /// Returns the head length relative to the start of `data`, once the terminator is found.
    pub(crate) fn scan(&mut self, data: &BytesView) -> Option<usize> {
        let mut offset = 0;
        for (slice, _meta) in data.slices() {
            for &byte in slice {
                offset += 1;
                self.matched = if byte == Self::PATTERN[self.matched] {
                    self.matched + 1
                } else {
                    usize::from(byte == b'\r')
                };

                if self.matched == Self::PATTERN.len() {
                    return Some(offset);
                }
            }
        }

        None
    }
}

/// Parses the status line and headers. Header values are zero-copy slices of `head`.
pub(crate) fn parse_head(head: &Bytes, headers: &mut HeaderMap) -> Result<StatusCode, HttpError> {
    let mut lines = HeadLines { head, position: 0 };

    let status_line = lines.next().ok_or_else(|| HttpError::validation("missing status line"))?;
    let status = head[status_line]
        .strip_prefix(b"HTTP/1.1 ")
        .and_then(|rest| rest.get(..3))
        .and_then(|code| StatusCode::from_bytes(code).ok())
        .ok_or_else(|| HttpError::validation("invalid status line"))?;

    for line in lines {
        let colon = head[line.clone()]
            .iter()
            .position(|&b| b == b':')
            .ok_or_else(|| HttpError::validation("invalid header line"))?;

        let name = resolve_header_name(&head[line.start..line.start + colon])?;

        let mut value = line.start + colon + 1..line.end;
        while value.start < value.end && head[value.start].is_ascii_whitespace() {
            value.start += 1;
        }
        while value.end > value.start && head[value.end - 1].is_ascii_whitespace() {
            value.end -= 1;
        }

        // `Bytes::slice` only bumps a reference count, so the value shares the head's memory.
        headers.append(name, shared_value(head.slice(value))?);
    }

    Ok(status)
}

/// Resolves a header name without allocating for standard and expected custom names.
fn resolve_header_name(name: &[u8]) -> Result<HeaderName, HttpError> {
    if name.eq_ignore_ascii_case(X_CORRELATION_ID.as_str().as_bytes()) {
        return Ok(X_CORRELATION_ID.clone());
    }

    // Standard names are recognized (case-insensitively) without allocating; other names are copied.
    HeaderName::from_bytes(name).map_err(malformed)
}

/// Iterates the byte ranges of the lines in an HTTP/1.1 head, stopping at the empty line.
#[derive(Debug)]
struct HeadLines<'a> {
    head: &'a [u8],
    position: usize,
}

impl Iterator for HeadLines<'_> {
    type Item = std::ops::Range<usize>;

    fn next(&mut self) -> Option<Self::Item> {
        let rest = self.head.get(self.position..)?;
        let len = rest.windows(2).position(|w| w == b"\r\n")?;
        let line = self.position..self.position + len;
        self.position = line.end + 2;
        (!line.is_empty()).then_some(line)
    }
}

/// Formats `value` as decimal digits into `buffer` without allocating.
pub(crate) fn format_decimal(mut value: u64, buffer: &mut [u8; 20]) -> &[u8] {
    let mut start = buffer.len();
    loop {
        start -= 1;
        let digit = u8::try_from(value % 10).expect("a single decimal digit always fits in u8");
        buffer[start] = b'0' + digit;
        value /= 10;
        if value == 0 {
            return &buffer[start..];
        }
    }
}
