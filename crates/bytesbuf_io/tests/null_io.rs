// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Null io through public APIs.

#![cfg(test)]

#[cfg(feature = "test-util")]
mod testing_null {
    use bytesbuf::mem::testing::TransparentMemory;
    use bytesbuf::mem::{CallbackMemory, Memory, OpaqueMemory};
    use bytesbuf::{BytesBuf, BytesView};
    use bytesbuf_io::testing::Null;
    use testing_aids::execute_or_terminate_process;

    #[test]
    fn smoke_test() {
        execute_or_terminate_process(|| {
            futures::executor::block_on(async {
                let mut s = Null::new();

                let buffer = s.reserve(1000);
                assert!(buffer.remaining_capacity() >= 1000);

                let (bytes_read, buffer) = s.read_at_most_into(100, buffer).await.unwrap();
                assert_eq!(bytes_read, 0);
                assert_eq!(buffer.len(), 0);

                let (bytes_read, buffer) = s.read_more_into(buffer).await.unwrap();
                assert_eq!(bytes_read, 0);
                assert_eq!(buffer.len(), 0);

                let mut buffer = s.read_any().await.unwrap();
                assert_eq!(buffer.len(), 0);

                s.write(buffer.consume_all()).await.unwrap();
            });
        });
    }

    #[test]
    fn default_returns_working_instance() {
        execute_or_terminate_process(|| {
            futures::executor::block_on(async {
                let mut s = Null::default();

                // Verify it can reserve memory
                let buffer = s.reserve(100);
                assert!(buffer.remaining_capacity() >= 100);

                // Verify read operations work
                let (bytes_read, _) = s.read_at_most_into(10, BytesBuf::new()).await.unwrap();
                assert_eq!(bytes_read, 0);

                // Verify write operations work
                let empty_view = BytesView::default();
                s.write(empty_view).await.unwrap();
            });
        });
    }

    #[test]
    fn memory_returns_configured_provider() {
        // The configured provider adds a distinctive marker offset to every reservation, so we can
        // confirm the stream returns this provider rather than a default one.
        const MARKER_OFFSET: usize = 7;

        let custom_memory = OpaqueMemory::new(CallbackMemory::new(TransparentMemory::new(), |inner, min_bytes| {
            inner.reserve(min_bytes + MARKER_OFFSET)
        }));

        let null_stream = Null::builder().memory(custom_memory).build();

        // Get memory from stream and use it
        let stream_memory = null_stream.memory();
        let buf = stream_memory.reserve(10);

        assert_eq!(
            buf.capacity(),
            10 + MARKER_OFFSET,
            "stream should return the configured provider, which applies a marker offset"
        );
    }
}
