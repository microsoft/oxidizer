// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The buffer that keeps the request or response body up to the configured limit in bytes.

use std::sync::{Arc, OnceLock};

use bytesbuf::mem::GlobalPool;
use bytesbuf::{BytesBuf, BytesView};
use observed::Text;

/// Default logged body size in bytes.
pub(crate) const DEFAULT_BODY_SIZE_LIMIT: usize = 32 * 1024;

/// The maximum supported value for `body_size_limit`.
pub(crate) const MAX_BODY_SIZE_LIMIT: usize = 1536 * 1024;

/// The pool of the body buffers, which all layers share.
static GLOBAL_POOL: OnceLock<GlobalPool> = OnceLock::new();

pub(crate) struct BodyBuffer {
    contents: BufferContents,
    size_limit: usize,
}

enum BufferContents {
    /// The bytes that arrived, up to the limit. A `BytesBuf` is 552 bytes, and the guard of every server request holds
    /// a buffer, so it is boxed.
    Bytes(Option<Box<BytesBuf>>),
    /// The text of a body whose bytes all came at once, made without copying them into `Bytes` first.
    Text(Text),
}

impl BodyBuffer {
    pub(crate) const fn new(size_limit: usize) -> Self {
        Self {
            contents: BufferContents::Bytes(None),
            size_limit,
        }
    }

    /// Pushes `data` into the buffer.
    pub(crate) fn push(&mut self, data: &BytesView) {
        let BufferContents::Bytes(slot) = &mut self.contents else {
            return;
        };
        let length = slot.as_ref().map_or(0, |bytes| bytes.len());
        let count = data.len().min(self.size_limit - length);
        if count == 0 {
            return;
        }
        // Room for the whole limit keeps the bytes of the body in one block, so its text is made from one slice.
        let bytes = slot.get_or_insert_with(|| Box::new(GLOBAL_POOL.get_or_init(GlobalPool::new).reserve(self.size_limit)));
        let mut left = count;
        for (slice, _) in data.slices() {
            if left == 0 {
                break;
            }
            let take = slice.len().min(left);
            bytes.put_slice(&slice[..take]);
            left -= take;
        }
    }

    /// Copies the last bytes of the body. If no bytes came before them, they are copied only into the text.
    pub(crate) fn push_last(&mut self, data: &BytesView) {
        match &self.contents {
            BufferContents::Bytes(None) => {
                self.contents = BufferContents::Text(body_text(data, self.size_limit));
            }
            _ => self.push(data),
        }
    }

    /// The text of the bytes, as `lossy_text` makes it.
    pub(crate) fn text(&self) -> Text {
        match &self.contents {
            BufferContents::Bytes(None) => Text::from(""),
            BufferContents::Bytes(Some(bytes)) => {
                let view = bytes.peek();
                // The bytes are in one slice, unless the limit needs more than one block of the pool.
                if view.first_slice().len() == view.len() {
                    lossy_text(view.first_slice(), self.size_limit)
                } else {
                    lossy_text(&view.to_vec(), self.size_limit)
                }
            }
            BufferContents::Text(text) => text.clone(),
        }
    }
}

/// The text of a body whose bytes are all in `body`, up to `size_limit` bytes.
pub(crate) fn body_text(body: &BytesView, size_limit: usize) -> Text {
    let length = body.len().min(size_limit);
    if let Some(bytes) = body.first_slice().get(..length) {
        return lossy_text(bytes, size_limit);
    }
    // The bytes are in several slices, so they need a buffer to be in one.
    let mut buffer = BodyBuffer::new(size_limit);
    buffer.push(body);
    buffer.text()
}

/// Converts bytes into lossy UTF-8 text. If the bytes reach `size_limit`, the text leaves out an incomplete character at
/// their end.
fn lossy_text(bytes: &[u8], size_limit: usize) -> Text {
    // #[gamma::skip(cond.always_false, tag = "equivalent", reason = "Empty bytes give the same empty text either way; the check only saves an allocation.")]
    if bytes.is_empty() {
        return Text::from("");
    }

    let bytes = if bytes.len() == size_limit {
        without_incomplete_character(bytes)
    } else {
        bytes
    };
    let text = match str::from_utf8(bytes) {
        Ok(text) => Arc::from(text),
        Err(_) => Arc::from(String::from_utf8_lossy(bytes)),
    };
    Text::from(text)
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
