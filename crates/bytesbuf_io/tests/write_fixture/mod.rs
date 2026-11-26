// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::convert::Infallible;

use bytesbuf::mem::testing::TransparentMemory;
use bytesbuf::mem::{HasMemory, Memory, MemoryShared};
use bytesbuf::{BytesBuf, BytesView};
use bytesbuf_io::Write;

#[derive(Debug)]
pub(crate) struct FakeWrite {
    contents: BytesView,
}

impl FakeWrite {
    pub(crate) fn new() -> Self {
        Self {
            contents: BytesView::new(),
        }
    }

    pub(crate) fn into_contents(self) -> BytesView {
        self.contents
    }
}

impl Memory for FakeWrite {
    fn reserve(&self, min_bytes: usize) -> BytesBuf {
        TransparentMemory::new().reserve(min_bytes)
    }
}

impl HasMemory for FakeWrite {
    fn memory(&self) -> impl MemoryShared {
        TransparentMemory::new()
    }
}

impl Write for FakeWrite {
    type Error = Infallible;

    async fn write(&mut self, data: BytesView) -> Result<(), Self::Error> {
        self.contents.append(data);
        Ok(())
    }
}
