// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::io::{self, BufRead, Read};

use crate::BytesView;

/// Adapter that implements [`Read`] and [`BufRead`] for [`BytesView`].
///
/// Create an instance via [`BytesView::reader()`][1].
///
/// Because [`BytesView`] is already buffered, this adapter implements [`BufRead`] directly
/// without needing an intermediate buffer. Prefer this over wrapping in [`std::io::BufReader`].
///
/// [1]: crate::BytesView::reader
#[derive(Debug)]
pub struct BytesViewReader<'b> {
    inner: &'b mut BytesView,
}

impl<'b> BytesViewReader<'b> {
    #[must_use]
    pub(crate) const fn new(inner: &'b mut BytesView) -> Self {
        Self { inner }
    }
}

impl Read for BytesViewReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.inner.is_empty() {
            return Ok(0);
        }

        let to_read = buf.len().min(self.inner.len());
        self.inner.copy_to_slice(&mut buf[..to_read]);
        Ok(to_read)
    }
}

impl BufRead for BytesViewReader<'_> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        Ok(self.inner.first_slice())
    }

    fn consume(&mut self, amount: usize) {
        self.inner.advance(amount);
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;
    use crate::mem::testing::TransparentMemory;

    #[test]
    fn smoke_test() {
        let memory = TransparentMemory::new();
        let mut view = BytesView::copied_from_slice(b"Hello, world", &memory);
        let mut reader = view.reader();

        let mut buffer = [0u8; 5];
        let bytes_read = reader.read(&mut buffer).unwrap();

        // We use white-box knowledge to know that we always read as much as is available,
        // there are no partial reads unless at end of data. This simplifies the test logic.
        assert_eq!(bytes_read, 5);
        assert_eq!(&buffer, b"Hello");

        let bytes_read = reader.read(&mut buffer).unwrap();

        assert_eq!(bytes_read, 5);
        assert_eq!(&buffer, b", wor");

        let bytes_read = reader.read(&mut buffer).unwrap();
        assert_eq!(bytes_read, 2);
        assert_eq!(&buffer[..2], b"ld");

        let bytes_read = reader.read(&mut buffer).unwrap();
        assert_eq!(bytes_read, 0);
    }

    #[test]
    fn buf_read_fill_buf_and_consume() {
        let memory = TransparentMemory::new();
        let mut view = BytesView::copied_from_slice(b"Hello, world", &memory);
        let mut reader = view.reader();

        // fill_buf returns the first contiguous slice without consuming it.
        let buf = reader.fill_buf().unwrap();
        assert_eq!(buf, b"Hello, world");

        // Calling fill_buf again returns the same data (no consumption).
        let buf = reader.fill_buf().unwrap();
        assert_eq!(buf, b"Hello, world");

        // Consume some bytes and verify the remainder.
        reader.consume(7);
        let buf = reader.fill_buf().unwrap();
        assert_eq!(buf, b"world");

        // Consume remaining bytes.
        reader.consume(5);
        let buf = reader.fill_buf().unwrap();
        assert!(buf.is_empty());
    }

    #[test]
    fn buf_read_read_line() {
        let memory = TransparentMemory::new();
        let mut view = BytesView::copied_from_slice(b"first\nsecond\n", &memory);
        let mut reader = view.reader();

        let mut line = String::new();
        let bytes_read = reader.read_line(&mut line).unwrap();
        assert_eq!(bytes_read, 6);
        assert_eq!(line, "first\n");

        line.clear();
        let bytes_read = reader.read_line(&mut line).unwrap();
        assert_eq!(bytes_read, 7);
        assert_eq!(line, "second\n");

        line.clear();
        let bytes_read = reader.read_line(&mut line).unwrap();
        assert_eq!(bytes_read, 0);
        assert!(line.is_empty());
    }

    #[test]
    fn buf_read_on_empty_view() {
        let memory = TransparentMemory::new();
        let mut view = BytesView::copied_from_slice(b"", &memory);
        let mut reader = view.reader();

        let buf = reader.fill_buf().unwrap();
        assert!(buf.is_empty());
    }
}
