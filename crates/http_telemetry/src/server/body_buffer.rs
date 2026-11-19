// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The buffer that keeps the request or response body up to the configured limit in bytes.

use std::sync::Arc;

use bytesbuf::BytesView;
use observed::Text;

pub(crate) struct BodyBuffer {
    bytes: Vec<u8>,
    limit: usize,
}

impl BodyBuffer {
    pub(crate) const fn new(limit: usize) -> Self {
        Self { bytes: Vec::new(), limit }
    }

    /// Copies the bytes of `data` that fit under the limit.
    pub(crate) fn push(&mut self, data: &BytesView) {
        // The buffer grows only with the bytes that arrive, because a client can declare any length.
        for (slice, _) in data.slices() {
            let room = self.limit - self.bytes.len();
            self.bytes.extend_from_slice(&slice[..slice.len().min(room)]);
        }
    }

    /// Convert bytes into lossy UTF-8 text. If the bytes reach the limit, the text leaves out an incomplete character at
    /// their end.
    pub(crate) fn text(&self) -> Text {
        // #[gamma::skip(cond.always_false, tag = "equivalent", reason = "Empty bytes give the same empty text either way; the check only saves an allocation.")]
        if self.bytes.is_empty() {
            return Text::from("");
        }

        let bytes = if self.bytes.len() == self.limit {
            without_incomplete_character(&self.bytes)
        } else {
            &self.bytes
        };
        let text = match str::from_utf8(bytes) {
            Ok(text) => Arc::from(text),
            Err(_) => Arc::from(String::from_utf8_lossy(bytes)),
        };
        Text::from(text)
    }
}

fn without_incomplete_character(bytes: &[u8]) -> &[u8] {
    // A character has at most 4 bytes, so an incomplete one has at most 3.
    // #[gamma::skip(literal.int_increment, tag = "equivalent", reason = "A longer end has the same last chunk, because an incomplete character has at most 3 bytes.")]
    let end = &bytes[bytes.len().saturating_sub(3)..];
    let incomplete = end
        .utf8_chunks()
        .last()
        .map(|chunk| chunk.invalid())
        .filter(|invalid| str::from_utf8(invalid).is_err_and(|error| error.error_len().is_none()))
        .map_or(0, <[u8]>::len);
    &bytes[..bytes.len() - incomplete]
}
