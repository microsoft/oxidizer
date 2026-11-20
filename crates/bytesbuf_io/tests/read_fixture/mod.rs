// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::convert::Infallible;
use std::num::NonZeroUsize;

use bytesbuf::mem::testing::TransparentMemory;
use bytesbuf::mem::{HasMemory, Memory, MemoryShared};
use bytesbuf::{BytesBuf, BytesView};
use bytesbuf_io::Read;

#[derive(Debug)]
pub(crate) struct FakeRead {
    contents: BytesView,
    max_read_size: usize,
}

impl FakeRead {
    pub(crate) fn new(contents: BytesView) -> Self {
        Self {
            contents,
            max_read_size: usize::MAX,
        }
    }

    pub(crate) fn builder() -> Self {
        Self::new(BytesView::new())
    }

    pub(crate) fn contents(mut self, contents: BytesView) -> Self {
        self.contents = contents;
        self
    }

    pub(crate) fn max_read_size(mut self, size: NonZeroUsize) -> Self {
        self.max_read_size = size.get();
        self
    }

    pub(crate) fn build(self) -> Self {
        self
    }
}

impl Memory for FakeRead {
    fn reserve(&self, min_bytes: usize) -> BytesBuf {
        TransparentMemory::new().reserve(min_bytes)
    }
}

impl HasMemory for FakeRead {
    fn memory(&self) -> impl MemoryShared {
        TransparentMemory::new()
    }
}

impl Read for FakeRead {
    type Error = Infallible;

    async fn read_at_most_into(&mut self, len: usize, mut into: BytesBuf) -> Result<(usize, BytesBuf), Self::Error> {
        let count = len.min(self.contents.len()).min(self.max_read_size);
        into.put_bytes(self.contents.range(..count));
        self.contents.advance(count);
        Ok((count, into))
    }

    async fn read_more_into(&mut self, into: BytesBuf) -> Result<(usize, BytesBuf), Self::Error> {
        self.read_at_most_into(100, into).await
    }

    async fn read_any(&mut self) -> Result<BytesBuf, Self::Error> {
        Ok(self.read_at_most_into(100, BytesBuf::new()).await?.1)
    }
}
