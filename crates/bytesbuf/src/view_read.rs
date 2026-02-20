// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::io::{self, BufRead, Read};

use crate::BytesView;

impl Read for BytesView {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
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

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use std::io::{BufRead, Read};

    use crate::BytesView;
    use crate::mem::testing::TransparentMemory;

    #[test]
    fn smoke_test() {
        let memory = TransparentMemory::new();
        let mut view = BytesView::copied_from_slice(b"Hello, world", &memory);

        let mut buffer = [0u8; 5];
        let bytes_read = view.read(&mut buffer).unwrap();

        // We use white-box knowledge to know that we always read as much as is available,
        // there are no partial reads unless at end of data. This simplifies the test logic.
        assert_eq!(bytes_read, 5);
        assert_eq!(&buffer, b"Hello");

        let bytes_read = view.read(&mut buffer).unwrap();

        assert_eq!(bytes_read, 5);
        assert_eq!(&buffer, b", wor");

        let bytes_read = view.read(&mut buffer).unwrap();
        assert_eq!(bytes_read, 2);
        assert_eq!(&buffer[..2], b"ld");

        let bytes_read = view.read(&mut buffer).unwrap();
        assert_eq!(bytes_read, 0);
    }

    #[test]
    fn buf_read_fill_buf_and_consume() {
        let memory = TransparentMemory::new();
        let mut view = BytesView::copied_from_slice(b"Hello, world", &memory);

        // fill_buf returns the first contiguous slice without consuming it.
        let buf = view.fill_buf().unwrap();
        assert_eq!(buf, b"Hello, world");

        // Calling fill_buf again returns the same data (no consumption).
        let buf = view.fill_buf().unwrap();
        assert_eq!(buf, b"Hello, world");

        // Consume some bytes and verify the remainder.
        BufRead::consume(&mut view, 7);
        let buf = view.fill_buf().unwrap();
        assert_eq!(buf, b"world");

        // Consume remaining bytes.
        BufRead::consume(&mut view, 5);
        let buf = view.fill_buf().unwrap();
        assert!(buf.is_empty());
    }

    #[test]
    fn buf_read_read_line() {
        let memory = TransparentMemory::new();
        let mut view = BytesView::copied_from_slice(b"first\nsecond\n", &memory);

        let mut line = String::new();
        let bytes_read = view.read_line(&mut line).unwrap();
        assert_eq!(bytes_read, 6);
        assert_eq!(line, "first\n");

        line.clear();
        let bytes_read = view.read_line(&mut line).unwrap();
        assert_eq!(bytes_read, 7);
        assert_eq!(line, "second\n");

        line.clear();
        let bytes_read = view.read_line(&mut line).unwrap();
        assert_eq!(bytes_read, 0);
        assert!(line.is_empty());
    }

    #[test]
    fn buf_read_on_empty_view() {
        let memory = TransparentMemory::new();
        let mut view = BytesView::copied_from_slice(b"", &memory);

        let buf = view.fill_buf().unwrap();
        assert!(buf.is_empty());
    }
}
