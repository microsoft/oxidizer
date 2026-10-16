// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! A mocked HTTP/1.1 connection.

use bytesbuf::BytesView;
use bytesbuf::mem::GlobalPool;

use crate::{BODY_LEN, CHUNK_LEN};

/// A mocked HTTP/1.1 connection. Each request is answered with a canned response that is
/// delivered in chunks of [`CHUNK_LEN`] bytes.
#[derive(Debug)]
pub(crate) struct MockConnection {
    response: BytesView,
    position: usize,
}

impl MockConnection {
    pub(crate) fn new(memory: &GlobalPool) -> Self {
        let head = format!(
            "HTTP/1.1 200 OK\r\n\
             Content-Type: application/octet-stream\r\n\
             Content-Length: {BODY_LEN}\r\n\
             Date: Thu, 01 Oct 2026 12:00:00 GMT\r\n\
             Server: mock/1.0\r\n\
             Cache-Control: no-store\r\n\
             ETag: \"5f3c-1a2b\"\r\n\
             Vary: Accept-Encoding\r\n\
             Connection: keep-alive\r\n\
             Content-Language: en-US\r\n\
             X-Correlation-Id: 0b7e2d4c-1a3f-4e5d-9c8b-6a7f0e1d2c3b\r\n\
             \r\n"
        );

        // The canned response lives in pooled memory, in a single block.
        let mut response = memory.reserve(head.len() + BODY_LEN);
        response.put_slice(head.as_bytes());
        response.put_byte_repeated(b'r', BODY_LEN);
        let response = response.consume_all();
        assert_eq!(
            response.first_slice().len(),
            response.len(),
            "a reservation smaller than the block size occupies a single span"
        );

        let position = response.len();
        Self { response, position }
    }

    /// Sends a request; the mocked server answers it with the canned response.
    pub(crate) fn write(&mut self, request: &BytesView) {
        assert!(request.len() > BODY_LEN, "the request must carry its body");
        self.position = 0;
    }

    /// Returns the next chunk of the response, or an empty slice once it is fully read.
    pub(crate) fn read(&mut self) -> &[u8] {
        let start = self.position;
        self.position = (start + CHUNK_LEN).min(self.response.len());
        &self.response.first_slice()[start..self.position]
    }
}
