// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Pending io through public APIs.

#![cfg(test)]

#[cfg(feature = "test-util")]
mod testing_pending {
    use bytesbuf::mem::testing::TransparentMemory;
    use bytesbuf::mem::{CallbackMemory, Memory, OpaqueMemory};
    use bytesbuf::{BytesBuf, BytesView};
    use bytesbuf_io::testing::Pending;
    use future_assert::FutureTestExt;

    #[test]
    fn smoke_test() {
        let stream = Pending::new();

        let reserved1 = stream.reserve(123);
        assert!(reserved1.capacity() >= 123);

        let memory = stream.memory();
        let reserved2 = memory.reserve(123);
        assert!(reserved2.capacity() >= 123);
    }

    #[test]
    fn default_returns_working_instance() {
        let stream = Pending::default();

        // Verify it can reserve memory
        let buffer = stream.reserve(100);
        assert!(buffer.remaining_capacity() >= 100);

        // Verify memory() works
        let stream_memory = stream.memory();
        let buffer2 = stream_memory.reserve(50);
        assert!(buffer2.remaining_capacity() >= 50);
    }

    #[test]
    fn memory_returns_configured_provider() {
        // The configured provider adds a distinctive marker offset to every reservation, so we can
        // confirm the stream returns this provider rather than a default one.
        const MARKER_OFFSET: usize = 7;

        let custom_memory = OpaqueMemory::new(CallbackMemory::new(TransparentMemory::new(), |inner, min_bytes| {
            inner.reserve(min_bytes + MARKER_OFFSET)
        }));

        let pending_stream = Pending::builder().memory(custom_memory).build();

        // Get memory from stream and use it
        let stream_memory = pending_stream.memory();
        let buf = stream_memory.reserve(10);

        assert_eq!(
            buf.capacity(),
            10 + MARKER_OFFSET,
            "stream should return the configured provider, which applies a marker offset"
        );
    }

    #[test]
    fn read_at_most_into_returns_pending_on_first_poll() {
        let mut stream = Pending::new();
        let buffer = BytesBuf::new();

        stream.read_at_most_into(100, buffer).unwrap_pending_for(1);
    }

    #[test]
    fn read_more_into_returns_pending_on_first_poll() {
        let mut stream = Pending::new();
        let buffer = BytesBuf::new();

        stream.read_more_into(buffer).unwrap_pending_for(1);
    }

    #[test]
    fn read_any_returns_pending_on_first_poll() {
        let mut stream = Pending::new();

        stream.read_any().unwrap_pending_for(1);
    }

    #[test]
    fn write_returns_pending_on_first_poll() {
        let mut stream = Pending::new();
        let data = BytesView::default();

        stream.write(data).unwrap_pending_for(1);
    }
}
