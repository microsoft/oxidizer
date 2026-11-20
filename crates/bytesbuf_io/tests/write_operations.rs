// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Write operations through public APIs.

#![cfg(test)]

mod write_fixture;

mod write_ext {
    use std::convert::Infallible;

    use bytesbuf::BytesView;
    use bytesbuf_io::WriteExt;
    use testing_aids::async_test;

    use crate::write_fixture::FakeWrite;

    const TEST_DATA: &[u8] = b"Hello, world!";

    #[test]
    fn prepare_and_write_writes_on_prepare_success() {
        async_test(async || {
            let mut stream = FakeWrite::new();

            stream
                .prepare_and_write(100, |mut buf| {
                    assert!(buf.capacity() >= 100);

                    buf.put_slice(TEST_DATA);
                    Ok::<BytesView, Infallible>(buf.consume_all())
                })
                .await
                .unwrap();

            let contents = stream.into_contents();

            assert_eq!(contents.len(), TEST_DATA.len());
        });
    }

    #[test]
    fn prepare_and_write_cancels_on_prepare_fail() {
        async_test(async || {
            let mut stream = FakeWrite::new();

            stream
                .prepare_and_write(100, |mut buf| {
                    assert!(buf.capacity() >= 100);

                    buf.put_slice(TEST_DATA);
                    Err(std::io::Error::other("oopsy-woopsie, serialization failed"))
                })
                .await
                .unwrap_err();

            let contents = stream.into_contents();

            assert_eq!(contents.len(), 0);
        });
    }
}
