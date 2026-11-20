// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::io::Write;

use crate::BytesBuf;
use crate::mem::Memory;

/// The minimum reservation size for a [`BytesBufWriter`] when the buffer has no existing capacity.
const INITIAL_RESERVATION_BYTES: usize = 4096;

/// The minimum reservation size for a [`BytesBufWriter`] when the buffer already has some capacity.
const GROWTH_RESERVATION_BYTES: usize = 65536;

/// Adapter that implements `std::io::Write` for [`BytesBuf`].
///
/// Create an instance via [`BytesBuf::into_writer()`][1].
///
/// The adapter will automatically extend the underlying [`BytesBuf`] as needed when writing
/// by allocating additional memory capacity from the memory provider `M`.
///
/// [1]: crate::BytesBuf::into_writer
#[derive(Debug)]
pub struct BytesBufWriter<M: Memory> {
    inner: BytesBuf,
    memory: M,
}

impl<M: Memory> BytesBufWriter<M> {
    #[must_use]
    pub(crate) const fn new(inner: BytesBuf, memory: M) -> Self {
        Self { inner, memory }
    }

    #[must_use]
    /// Returns the wrapped [`BytesBuf`].
    pub fn into_inner(self) -> BytesBuf {
        self.inner
    }

    /// Ensures the buffer has enough remaining capacity for `required_bytes`, reserving more
    /// memory from the memory provider if needed.
    ///
    /// The `Write` trait is designed for piece-by-piece writing in small increments, so callers
    /// will typically make many small writes. Naively reserving only the exact amount requested
    /// each time would cause excessive memory allocations. Instead, we reserve ahead because
    /// more data is almost certainly coming.
    ///
    /// The reservation strategy balances between avoiding waste for small payloads (e.g. short
    /// JSON error responses, where over-reserving could open a door to resource exhaustion) and
    /// reducing allocation frequency for large payloads:
    ///
    /// * **Empty buffer, small payload (at most 4 KiB):** reserve 4 KiB. We assume the total data
    ///   will be modest, so we keep the initial allocation small.
    /// * **Otherwise:** reserve at least 64 KiB (or the payload size if larger). Once data
    ///   crosses the 4 KiB boundary we are likely dealing with a large payload, so we take big
    ///   bites to reduce the frequency of memory reservations.
    fn ensure_sufficient_capacity(&mut self, required_bytes: usize) {
        if self.inner.remaining_capacity() >= required_bytes {
            return;
        }

        let reservation = if self.inner.capacity() == 0 && required_bytes <= INITIAL_RESERVATION_BYTES {
            INITIAL_RESERVATION_BYTES
        } else {
            required_bytes.max(GROWTH_RESERVATION_BYTES)
        };

        self.inner.reserve(reservation, &self.memory);
    }
}

impl<M: Memory> Write for BytesBufWriter<M> {
    #[inline]
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.ensure_sufficient_capacity(buf.len());
        self.inner.put_slice(buf);
        Ok(buf.len())
    }

    #[inline]
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    // Exact reservation thresholds are private policy rather than part of the public writer contract.
    use super::*;
    use crate::mem::testing::TransparentMemory;

    #[test]
    fn empty_buffer_small_write_reserves_initial_size() {
        let memory = TransparentMemory::new();
        let buf = BytesBuf::new();
        assert_eq!(buf.capacity(), 0);

        let mut writer = buf.into_writer(&memory);
        writer.write_all(b"hello").unwrap();

        let buf = writer.into_inner();
        assert!(
            buf.capacity() >= INITIAL_RESERVATION_BYTES,
            "expected capacity >= {}, got {}",
            INITIAL_RESERVATION_BYTES,
            buf.capacity()
        );
    }

    #[test]
    fn empty_buffer_large_write_reserves_growth_size() {
        let memory = TransparentMemory::new();
        let buf = BytesBuf::new();
        let payload = vec![0u8; INITIAL_RESERVATION_BYTES + 1];

        let mut writer = buf.into_writer(&memory);
        writer.write_all(&payload).unwrap();

        let buf = writer.into_inner();
        assert!(
            buf.capacity() >= GROWTH_RESERVATION_BYTES,
            "expected capacity >= {}, got {}",
            GROWTH_RESERVATION_BYTES,
            buf.capacity()
        );
    }

    #[test]
    fn non_empty_buffer_reserves_growth_size() {
        let memory = TransparentMemory::new();
        let mut buf = memory.reserve(16);
        buf.put_slice([1; 16]);
        assert_eq!(buf.remaining_capacity(), 0);
        assert!(buf.capacity() > 0);

        let mut writer = buf.into_writer(&memory);
        writer.write_all(b"more data").unwrap();

        let buf = writer.into_inner();
        assert!(
            buf.capacity() >= 16 + GROWTH_RESERVATION_BYTES,
            "expected capacity >= {}, got {}",
            16 + GROWTH_RESERVATION_BYTES,
            buf.capacity()
        );
    }
}
