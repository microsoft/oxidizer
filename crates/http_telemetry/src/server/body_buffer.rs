// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The buffer that keeps the start of a request or response body.

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

    /// Convert bytes into lossy UTF-8 text.
    pub(crate) fn text(&self) -> Text {
        // #[gamma::skip(cond.always_false, tag = "equivalent", reason = "Empty bytes give the same empty text either way; the check only saves an allocation.")]
        if self.bytes.is_empty() {
            return Text::from("");
        }

        let text = match str::from_utf8(&self.bytes) {
            Ok(text) => Arc::from(text),
            Err(_) => Arc::from(String::from_utf8_lossy(&self.bytes)),
        };
        Text::from(text)
    }
}
