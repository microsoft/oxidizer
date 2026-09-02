// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The contract every compressor and decompressor implements.
//!
//! [`Compression`] is what makes the formats interchangeable: the same push/pull state machine
//! whichever engine is behind it, with the [`Mode`][Compression::Mode] associated type recording
//! which direction an implementation runs in. [`Compressing`] and [`Decompressing`] add the
//! operations that only make sense in one direction.
//!
//! Everything here is re-exported at the crate root, so `compressors::core::Compression` and
//! `compressors::core::Compression` name the same trait.

use std::fmt;

use bytesbuf::{BytesBuf, BytesView};

use crate::error::Result;
use crate::output::Output;

pub(crate) mod sealed {
    /// Restricts [`Compression`][super::Compression] to this crate's own implementations.
    ///
    /// Each format module implements this for its compressor and decompressor beside the real
    /// implementation, so adding a format needs no edit here.
    pub trait Compression {}

    impl<D> Compression for Box<dyn super::Compression<Mode = D>> {}
    impl Compression for Box<dyn super::Compressing> {}
    impl Compression for Box<dyn super::Decompressing> {}
}

/// Marks a [`Compression`] implementation that compresses its input.
///
/// This marker cannot be constructed outside this crate.
#[derive(Debug)]
#[non_exhaustive]
pub struct Compress;

/// Marks a [`Compression`] implementation that decompresses its input.
///
/// This marker cannot be constructed outside this crate.
#[derive(Debug)]
#[non_exhaustive]
pub struct Decompress;

/// A streaming compression or decompression operation.
///
/// Every format's compressor and decompressor implements this contract. The `Mode` associated type
/// records which operation an implementation performs without changing how callers drive it. This
/// allows shared processing code to accept any `Compression`, while APIs that require one direction
/// can use `Compression<Mode = Compress>` or `Compression<Mode = Decompress>`.
///
/// The trait is sealed so formats and methods can be added without breaking downstream code.
/// Every implementation is `Send + Sync`.
///
/// # Examples
///
/// ```
/// use bytesbuf::BytesView;
/// use bytesbuf::mem::{GlobalPool, MemoryShared};
/// use compressors::core::{Compress, Compression};
/// use compressors::{Output, Resources, gzip};
///
/// fn compress(
///     mut compression: impl Compression<Mode = Compress>,
///     input: BytesView,
/// ) -> compressors::Result<BytesView> {
///     compression.process(input)
/// }
///
/// let memory = GlobalPool::new();
/// let compressed = compress(
///     gzip::Compressor::new(&Resources::default()),
///     BytesView::copied_from_slice(b"format agnostic", &memory),
/// )?;
///
/// assert_eq!(compressed.range(0..2).to_vec(), vec![0x1f, 0x8b]);
/// # Ok::<(), compressors::Error>(())
/// ```
pub trait Compression: sealed::Compression + fmt::Debug + Send + Sync {
    /// Whether this implementation compresses or decompresses its input.
    type Mode;

    /// Supplies more input.
    ///
    /// # Errors
    ///
    /// Returns an error if input is still pending or end of input has been signaled.
    fn push(&mut self, input: BytesView) -> Result<()>;

    /// Signals that no further input will be supplied.
    fn end_input(&mut self);

    /// Produces the next output chunk.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying engine fails or the input is invalid.
    fn pull(&mut self) -> Result<Output>;

    /// The number of bytes consumed from the input so far.
    fn total_in(&self) -> u64;

    /// The number of bytes produced so far.
    fn total_out(&self) -> u64;

    /// Processes one complete input and returns the whole result.
    ///
    /// This is shorthand for [`push`][Compression::push], [`end_input`][Compression::end_input], and
    /// draining [`pull`][Compression::pull]. It ends the operation, so an implementation serves
    /// one call. Drive `pull` directly to keep memory bounded by the configured chunk size.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying engine fails or the input is invalid.
    fn process(mut self, input: BytesView) -> Result<BytesView>
    where
        Self: Sized,
    {
        self.push(input)?;
        self.end_input();

        let mut collected = BytesBuf::new();
        loop {
            match self.pull()? {
                Output::Data(chunk) => collected.put_bytes(chunk),
                Output::Progress => {}
                Output::Done => break,
                Output::NeedInput => {
                    return Err(crate::Error::invalid_state("the operation requested input after end of input"));
                }
            }
        }

        Ok(collected.consume_all())
    }

    /// Compresses one complete input and returns the whole result.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying compression engine fails.
    fn compress(self, input: BytesView) -> Result<BytesView>
    where
        Self: Sized + Compression<Mode = Compress>,
    {
        self.process(input)
    }

    /// Decompresses one complete input and returns the whole result.
    ///
    /// # Errors
    ///
    /// Returns an error if the data is invalid, truncated, or exceeds the configured limits.
    fn decompress(self, input: BytesView) -> Result<BytesView>
    where
        Self: Sized + Compression<Mode = Decompress>,
    {
        self.process(input)
    }
}

/// Additional operations available while compressing.
pub trait Compressing: Compression<Mode = Compress> {
    /// Requests a resumable flush of all input supplied so far.
    ///
    /// Drain [`Compression::pull`] until it reports [`Output::NeedInput`] before pushing more
    /// input. Flushing can reduce the compression ratio.
    ///
    /// # Errors
    ///
    /// Returns an invalid-state error after end of input or a previous operation failure.
    fn flush(&mut self) -> Result<()>;
}

/// Additional operations available while decompressing.
pub trait Decompressing: Compression<Mode = Decompress> {
    /// Takes bytes already buffered after a completed single compressed stream.
    ///
    /// # Errors
    ///
    /// Returns an invalid-state error until decompression reports [`Output::Done`].
    fn take_remainder(&mut self) -> Result<BytesView>;
}

impl<D> Compression for Box<dyn Compression<Mode = D>> {
    type Mode = D;

    fn push(&mut self, input: BytesView) -> Result<()> {
        (**self).push(input)
    }

    fn end_input(&mut self) {
        (**self).end_input();
    }

    fn pull(&mut self) -> Result<Output> {
        (**self).pull()
    }

    fn total_in(&self) -> u64 {
        (**self).total_in()
    }

    fn total_out(&self) -> u64 {
        (**self).total_out()
    }
}

impl Compression for Box<dyn Compressing> {
    type Mode = Compress;

    fn push(&mut self, input: BytesView) -> Result<()> {
        (**self).push(input)
    }

    fn end_input(&mut self) {
        (**self).end_input();
    }

    fn pull(&mut self) -> Result<Output> {
        (**self).pull()
    }

    fn total_in(&self) -> u64 {
        (**self).total_in()
    }

    fn total_out(&self) -> u64 {
        (**self).total_out()
    }
}

impl Compressing for Box<dyn Compressing> {
    fn flush(&mut self) -> Result<()> {
        (**self).flush()
    }
}

impl Compression for Box<dyn Decompressing> {
    type Mode = Decompress;

    fn push(&mut self, input: BytesView) -> Result<()> {
        (**self).push(input)
    }

    fn end_input(&mut self) {
        (**self).end_input();
    }

    fn pull(&mut self) -> Result<Output> {
        (**self).pull()
    }

    fn total_in(&self) -> u64 {
        (**self).total_in()
    }

    fn total_out(&self) -> u64 {
        (**self).total_out()
    }
}

impl Decompressing for Box<dyn Decompressing> {
    fn take_remainder(&mut self) -> Result<BytesView> {
        (**self).take_remainder()
    }
}

/// A fixture that only ever reports progress, for exercising callers that must keep polling rather
/// than treat a progress step as output.
#[cfg(all(test, feature = "futures-stream"))]
#[derive(Debug)]
pub(crate) struct ProgressCompression {
    pulls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

#[cfg(all(test, feature = "futures-stream"))]
impl ProgressCompression {
    pub(crate) fn new(pulls: std::sync::Arc<std::sync::atomic::AtomicUsize>) -> Self {
        Self { pulls }
    }
}

#[cfg(all(test, feature = "futures-stream"))]
impl sealed::Compression for ProgressCompression {}

#[cfg(all(test, feature = "futures-stream"))]
impl Compression for ProgressCompression {
    type Mode = Compress;

    fn push(&mut self, _input: BytesView) -> Result<()> {
        Ok(())
    }

    fn end_input(&mut self) {}

    fn pull(&mut self) -> Result<Output> {
        self.pulls.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(Output::Progress)
    }

    // No caller on the path this fixture exists for asks for the byte counters; they are here only
    // because the trait requires them.
    #[cfg_attr(coverage_nightly, coverage(off))]
    fn total_in(&self) -> u64 {
        0
    }

    #[cfg_attr(coverage_nightly, coverage(off))]
    fn total_out(&self) -> u64 {
        0
    }
}

/// A fixture that always asks for input and always rejects it, for exercising callers that must
/// propagate a `push` failure rather than the specific reasons a real codec's `push` can fail.
#[cfg(all(test, feature = "futures-stream"))]
#[derive(Debug)]
pub(crate) struct RejectsPush;

#[cfg(all(test, feature = "futures-stream"))]
impl sealed::Compression for RejectsPush {}

#[cfg(all(test, feature = "futures-stream"))]
impl Compression for RejectsPush {
    type Mode = Compress;

    fn push(&mut self, _input: BytesView) -> Result<()> {
        Err(crate::Error::invalid_state("this fixture always rejects pushed input"))
    }

    fn end_input(&mut self) {}

    fn pull(&mut self) -> Result<Output> {
        Ok(Output::NeedInput)
    }

    // No caller on the path this fixture exists for asks for the byte counters; they are here only
    // because the trait requires them.
    #[cfg_attr(coverage_nightly, coverage(off))]
    fn total_in(&self) -> u64 {
        0
    }

    #[cfg_attr(coverage_nightly, coverage(off))]
    fn total_out(&self) -> u64 {
        0
    }
}
#[cfg(test)]
mod tests {
    use bytesbuf::mem::GlobalPool;

    use super::*;

    fn view(bytes: &[u8]) -> BytesView {
        BytesView::copied_from_slice(bytes, &GlobalPool::new())
    }

    #[test]
    fn process_forwards_progress_without_producing_data() {
        #[derive(Debug)]
        struct ProgressOnceThenDone {
            done: bool,
        }

        impl sealed::Compression for ProgressOnceThenDone {}

        impl Compression for ProgressOnceThenDone {
            type Mode = Compress;

            fn push(&mut self, _input: BytesView) -> Result<()> {
                Ok(())
            }

            fn end_input(&mut self) {}

            fn pull(&mut self) -> Result<Output> {
                if self.done {
                    return Ok(Output::Done);
                }

                self.done = true;
                Ok(Output::Progress)
            }
            // No caller on the path under test asks for the byte counters; they exist only because
            // the trait requires them.
            #[cfg_attr(coverage_nightly, coverage(off))]
            fn total_in(&self) -> u64 {
                0
            }

            #[cfg_attr(coverage_nightly, coverage(off))]
            fn total_out(&self) -> u64 {
                0
            }
        }

        let result = ProgressOnceThenDone { done: false }
            .process(view(b"ignored"))
            .expect("process succeeds even when a step only makes progress");

        assert!(result.is_empty(), "the fixture never reports data");
    }

    #[test]
    fn process_rejects_a_pull_that_still_requests_input_after_end() {
        #[derive(Debug)]
        struct NeedsMoreForever;

        impl sealed::Compression for NeedsMoreForever {}

        impl Compression for NeedsMoreForever {
            type Mode = Compress;

            fn push(&mut self, _input: BytesView) -> Result<()> {
                Ok(())
            }

            fn end_input(&mut self) {}

            fn pull(&mut self) -> Result<Output> {
                Ok(Output::NeedInput)
            }
            // No caller on the path under test asks for the byte counters; they exist only because
            // the trait requires them.
            #[cfg_attr(coverage_nightly, coverage(off))]
            fn total_in(&self) -> u64 {
                0
            }

            #[cfg_attr(coverage_nightly, coverage(off))]
            fn total_out(&self) -> u64 {
                0
            }
        }

        let error = NeedsMoreForever
            .process(view(b"ignored"))
            .expect_err("process rejects a pull that still requests input after end of input");
        assert!(error.is_invalid_state());
    }
}
