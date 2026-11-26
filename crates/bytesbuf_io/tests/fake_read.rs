// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Fake read through public APIs.

#![cfg(test)]

#[cfg(feature = "test-util")]
mod testing_fake_read {
    use bytesbuf::BytesView;
    use bytesbuf::mem::{GlobalPool, Memory};
    use bytesbuf_io::ReadExt;
    use bytesbuf_io::testing::FakeRead;
    use new_zealand::nz;
    use testing_aids::async_test;

    #[test]
    fn smoke_test() {
        async_test(async || {
            let memory = GlobalPool::new();
            let mut buf = memory.reserve(100);
            buf.put_byte(1);
            buf.put_byte(2);
            buf.put_byte(3);

            let contents = buf.consume_all();

            let mut read_stream = FakeRead::new(contents);

            let stream_memory = read_stream.reserve(1234);
            assert!(stream_memory.capacity() >= 1234);

            let mut payload = read_stream.read_exactly(3).await.unwrap();
            assert_eq!(payload.len(), 3);

            assert_eq!(payload.get_byte(), 1);
            assert_eq!(payload.get_byte(), 2);
            assert_eq!(payload.get_byte(), 3);

            let final_read = read_stream.read_any().await.unwrap();
            assert_eq!(final_read.len(), 0);
        });
    }

    #[test]
    fn with_max_read_size() {
        async_test(async || {
            let memory = GlobalPool::new();
            let test_data = BytesView::copied_from_slice(b"Hello, world!", &memory);
            let mut read_stream = FakeRead::builder().contents(test_data).max_read_size(nz!(2)).build();

            // He
            let mut payload = read_stream.read_any().await.unwrap().consume_all();
            assert_eq!(payload.len(), 2);
            assert_eq!(payload.get_byte(), b'H');
            assert_eq!(payload.get_byte(), b'e');

            // ll
            payload = read_stream.read_any().await.unwrap().consume_all();
            assert_eq!(payload.len(), 2);
            assert_eq!(payload.get_byte(), b'l');
            assert_eq!(payload.get_byte(), b'l');

            // o,
            payload = read_stream.read_any().await.unwrap().consume_all();
            assert_eq!(payload.len(), 2);
            assert_eq!(payload.get_byte(), b'o');
            assert_eq!(payload.get_byte(), b',');

            //  w
            payload = read_stream.read_any().await.unwrap().consume_all();
            assert_eq!(payload.len(), 2);
            assert_eq!(payload.get_byte(), b' ');
            assert_eq!(payload.get_byte(), b'w');

            // or
            payload = read_stream.read_any().await.unwrap().consume_all();
            assert_eq!(payload.len(), 2);
            assert_eq!(payload.get_byte(), b'o');
            assert_eq!(payload.get_byte(), b'r');

            // ld
            payload = read_stream.read_any().await.unwrap().consume_all();
            assert_eq!(payload.len(), 2);
            assert_eq!(payload.get_byte(), b'l');
            assert_eq!(payload.get_byte(), b'd');

            // !
            payload = read_stream.read_any().await.unwrap().consume_all();
            assert_eq!(payload.len(), 1);
            assert_eq!(payload.get_byte(), b'!');
        });
    }

    #[test]
    fn read_more() {
        async_test(async || {
            let memory = GlobalPool::new();
            let test_data = BytesView::copied_from_slice(b"Hello, world!", &memory);
            let mut read_stream = FakeRead::builder().contents(test_data.clone()).max_read_size(nz!(2)).build();

            let mut payload_buffer = read_stream.reserve(100);

            loop {
                let previous_len = payload_buffer.len();

                let (bytes_read, new_payload_buffer) = read_stream.read_more_into(payload_buffer).await.unwrap();

                payload_buffer = new_payload_buffer;

                // Sanity check.
                assert_eq!(previous_len + bytes_read, payload_buffer.len());

                if bytes_read == 0 {
                    break;
                }
            }

            assert_eq!(payload_buffer.len(), test_data.len());

            let mut payload = payload_buffer.consume_all();
            assert_eq!(payload.len(), test_data.len());
            assert_eq!(payload.get_byte(), b'H');
            assert_eq!(payload.get_byte(), b'e');
            assert_eq!(payload.get_byte(), b'l');
            assert_eq!(payload.get_byte(), b'l');
            assert_eq!(payload.get_byte(), b'o');
            assert_eq!(payload.get_byte(), b',');
            assert_eq!(payload.get_byte(), b' ');
            assert_eq!(payload.get_byte(), b'w');
            assert_eq!(payload.get_byte(), b'o');
            assert_eq!(payload.get_byte(), b'r');
            assert_eq!(payload.get_byte(), b'l');
            assert_eq!(payload.get_byte(), b'd');
            assert_eq!(payload.get_byte(), b'!');
            assert_eq!(payload.len(), 0);
        });
    }

    #[test]
    fn memory_returns_configured_provider() {
        use bytesbuf::mem::testing::TransparentMemory;
        use bytesbuf::mem::{CallbackMemory, OpaqueMemory};

        // The configured provider adds a distinctive marker offset to every reservation, so we can
        // confirm the stream returns this provider rather than a default one.
        const MARKER_OFFSET: usize = 7;

        let custom_memory = OpaqueMemory::new(CallbackMemory::new(TransparentMemory::new(), |inner, min_bytes| {
            inner.reserve(min_bytes + MARKER_OFFSET)
        }));

        let memory = GlobalPool::new();
        let contents = BytesView::copied_from_slice(b"test", &memory);
        let read_stream = FakeRead::builder().contents(contents).memory(custom_memory).build();

        // Get memory from stream and use it
        let stream_memory = read_stream.memory();
        let buf = stream_memory.reserve(10);

        assert_eq!(
            buf.capacity(),
            10 + MARKER_OFFSET,
            "stream should return the configured provider, which applies a marker offset"
        );
    }
}
