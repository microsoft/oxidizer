// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::io::Write;

use bytes::BufMut;

use crate::{BytesBuf, Memory};

/// Adapts a [`BytesBuf`] to implement the `std::io::Write` trait.
///
/// Instances are created via [`BytesBuf::as_write()`][1].
///
/// The adapter will automatically extend the underlying sequence builder as needed when writing
/// by allocating additional memory capacity from the memory provider `M`.
///
/// [1]: crate::BytesBuf::as_write
#[derive(Debug)]
pub(crate) struct BytesBufWrite<'sb, 'm, M: Memory> {
    inner: &'sb mut BytesBuf,
    memory: &'m M,
}

impl<'sb, 'm, M: Memory> BytesBufWrite<'sb, 'm, M> {
    #[must_use]
    pub(crate) const fn new(inner: &'sb mut BytesBuf, memory: &'m M) -> Self {
        Self { inner, memory }
    }
}

impl<M: Memory> Write for BytesBufWrite<'_, '_, M> {
    #[inline]
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.inner.reserve(buf.len(), self.memory);
        self.inner.put(buf);
        Ok(buf.len())
    }

    #[cfg_attr(test, mutants::skip)] // No-op, nothing to test.
    #[inline]
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use new_zealand::nz;

    use super::*;
    use crate::{FixedBlockTestMemory, TransparentTestMemory};

    #[test]
    fn smoke_test_write_and_verify_data() {
        let test_data = b"Hello, world! This is a test.";

        let memory = TransparentTestMemory::new();
        let mut builder = memory.reserve(100);

        {
            let mut write_adapter = builder.as_write(&memory);

            // no-op
            write_adapter.flush().unwrap();

            let bytes_written = write_adapter.write(test_data).expect("write should succeed");
            assert_eq!(bytes_written, test_data.len());
        }

        let sequence = builder.consume_all();
        assert_eq!(sequence, test_data.as_slice());
    }

    #[test]
    fn existing_content_is_preserved() {
        let memory = TransparentTestMemory::new();
        let mut builder = memory.reserve(100);

        // Add some initial content to the builder
        let initial_data = b"Initial content";
        builder.put_slice(initial_data);

        {
            let mut write_adapter = builder.as_write(&memory);

            let additional_data = b" - Additional data";
            let bytes_written = write_adapter.write(additional_data).expect("write should succeed");
            assert_eq!(bytes_written, additional_data.len());
        }

        // Verify both initial and additional data are present
        let sequence = builder.consume_all();
        let expected = b"Initial content - Additional data";
        assert_eq!(sequence, expected.as_slice());
    }

    #[test]
    fn sufficient_capacity_not_extended() {
        let test_data = b"Small data"; // Much smaller than 100 bytes

        let memory = FixedBlockTestMemory::new(nz!(1024));
        let mut builder = memory.reserve(100);

        let initial_capacity = builder.capacity();
        assert!(initial_capacity >= 100);

        {
            let mut write_adapter = builder.as_write(&memory);

            // Write data that fits within existing capacity
            let bytes_written = write_adapter.write(test_data).expect("write should succeed");
            assert_eq!(bytes_written, test_data.len());
        }

        // Verify capacity hasn't changed (no new allocation needed)
        assert_eq!(builder.capacity(), initial_capacity);

        // Verify the data is there
        let sequence = builder.consume_all();
        assert_eq!(sequence, test_data.as_slice());
    }
}
