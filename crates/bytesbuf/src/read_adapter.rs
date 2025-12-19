// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::io::{self, Read};

use crate::BytesView;

/// Adapter that implements `std::io::Read` for [`BytesView`].
///
/// Create an instance via [`BytesView::as_read()`][1].
///
/// [1]: crate::BytesView::as_read
#[derive(Debug)]
pub(crate) struct BytesViewReader<'b> {
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

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;
    use crate::TransparentTestMemory;

    #[test]
    fn smoke_test() {
        let memory = TransparentTestMemory::new();
        let mut view = BytesView::copied_from_slice(b"Hello, world", &memory);
        let mut reader = view.as_read();

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
}
