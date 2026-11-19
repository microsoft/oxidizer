// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::io::{self, BufRead, Read};

use crate::BytesView;

impl Read for BytesView {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        // #[gamma::skip(cond.always_false, reason = "For an empty view to_read is zero; copying an empty prefix is a no-op and the same Ok(0) is returned")]
        if self.is_empty() {
            return Ok(0);
        }

        let to_read = buf.len().min(self.len());
        self.copy_to_slice(&mut buf[..to_read]);
        Ok(to_read)
    }
}

/// Because [`BytesView`] is already buffered, it implements [`BufRead`] directly
/// without needing an intermediate buffer. Prefer this over wrapping in [`std::io::BufReader`].
impl BufRead for BytesView {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        Ok(self.first_slice())
    }

    fn consume(&mut self, amount: usize) {
        self.advance(amount);
    }
}
