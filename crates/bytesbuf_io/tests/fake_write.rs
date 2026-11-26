// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Fake write through public APIs.

#![cfg(test)]

#[cfg(feature = "test-util")]
mod testing_fake_write {
    use std::convert::Infallible;

    use bytesbuf::BytesView;
    use bytesbuf::mem::testing::TransparentMemory;
    use bytesbuf::mem::{CallbackMemory, Memory, OpaqueMemory};
    use bytesbuf_io::WriteExt;
    use bytesbuf_io::testing::FakeWrite;
    use testing_aids::async_test;

    #[test]
    fn smoke_test() {
        async_test(async || {
            let mut write_stream = FakeWrite::new();

            write_stream
                .prepare_and_write(1234, |mut buf| {
                    buf.put_byte(1);
                    buf.put_byte(2);
                    buf.put_byte(3);
                    Ok::<BytesView, Infallible>(buf.consume_all())
                })
                .await
                .unwrap();

            assert_eq!(write_stream.contents().len(), 3);

            let mut contents = write_stream.into_contents();
            assert_eq!(contents.len(), 3);

            assert_eq!(contents.get_byte(), 1);
            assert_eq!(contents.get_byte(), 2);
            assert_eq!(contents.get_byte(), 3);
            assert_eq!(contents.len(), 0);
        });
    }

    #[test]
    fn default_returns_working_instance() {
        async_test(async || {
            let mut write_stream = FakeWrite::default();

            write_stream
                .prepare_and_write(10, |mut buf| {
                    buf.put_byte(42);
                    Ok::<BytesView, Infallible>(buf.consume_all())
                })
                .await
                .unwrap();

            assert_eq!(write_stream.contents().len(), 1);

            let mut contents = write_stream.into_contents();
            assert_eq!(contents.get_byte(), 42);
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

        let write_stream = FakeWrite::builder().memory(custom_memory).build();

        // Get memory from stream and use it
        let stream_memory = write_stream.memory();
        let buf = stream_memory.reserve(10);

        assert_eq!(
            buf.capacity(),
            10 + MARKER_OFFSET,
            "stream should return the configured provider, which applies a marker offset"
        );
    }
}
