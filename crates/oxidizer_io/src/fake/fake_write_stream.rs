// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::WriteStream;
use crate::mem::{FakeMemoryProvider, ProvideMemory, Sequence, SequenceBuilder};

/// A `WriteStream` implementation that writes to fake I/O memory.
/// For test and example purposes only, not for real I/O.
#[derive(Debug, Default)]
pub struct FakeWriteStream {
    inner: SequenceBuilder,
}

impl FakeWriteStream {
    // Trivial code for testing and examples, do not spend mutation time on this.
    #[cfg_attr(test, mutants::skip)]
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Transforms the instance into the inner sequence builder that received all the written data.
    // Trivial code for testing and examples, do not spend mutation time on this.
    #[cfg_attr(test, mutants::skip)]
    #[must_use]
    pub fn into_inner(self) -> SequenceBuilder {
        self.inner
    }

    /// Inspects the inner sequence builder that written data is stored in.
    // Trivial code for testing and examples, do not spend mutation time on this.
    #[cfg_attr(test, mutants::skip)]
    #[must_use]
    pub const fn inner(&self) -> &SequenceBuilder {
        &self.inner
    }
}

impl WriteStream for FakeWriteStream {
    // Trivial code for testing and examples, do not spend mutation time on this.
    #[cfg_attr(test, mutants::skip)]
    async fn write(&mut self, sequence: Sequence) -> crate::Result<()> {
        self.inner.append(sequence);
        Ok(())
    }
}

impl ProvideMemory for FakeWriteStream {
    // Trivial code for testing and examples, do not spend mutation time on this.
    #[cfg_attr(test, mutants::skip)]
    fn reserve(&self, min_bytes: usize) -> SequenceBuilder {
        FakeMemoryProvider.reserve(min_bytes)
    }
}

#[cfg(test)]
mod tests {
    use bytes::{Buf, BufMut};

    use super::*;
    use crate::WriteStreamExt;
    use crate::testing::async_test;

    #[test]
    fn smoke_test() {
        async_test! {
            let mut write_stream = FakeWriteStream::new();

            write_stream
                .prepare_and_write(1234, |mut sb| {
                    sb.put_u8(1);
                    sb.put_u8(2);
                    sb.put_u8(3);
                    Ok(sb.consume_all())
                })
                .await
                .unwrap();

            let mut sequence = write_stream.into_inner().consume_all();
            assert_eq!(sequence.len(), 3);

            assert_eq!(sequence.get_u8(), 1);
            assert_eq!(sequence.get_u8(), 2);
            assert_eq!(sequence.get_u8(), 3);
            assert_eq!(sequence.len(), 0);
        }
    }
}