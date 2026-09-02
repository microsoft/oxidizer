// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The contract every compressor and decompressor implements.
//!
//! [`Compression`] is what makes the formats interchangeable: the same push/pull state machine
//! whichever engine is behind it, with the [`Mode`][Compression::Mode] associated type recording
//! which direction an implementation runs in.
//!
//! One trait covers both directions. [`Compress`] and [`Decompress`] are what an API names when it
//! needs one of them -- `Compression<Mode = Compress>` accepts any compressor and no decompressor.
//!
//! [`Output`] is what one step of that contract reports, so it lives here too.

use std::fmt;

use bytesbuf::{BytesBuf, BytesView};

use crate::error::Result;

mod output;

pub use output::Output;

pub(crate) mod sealed {
    /// Restricts [`Compression`][super::Compression] to this crate's own implementations.
    ///
    /// Each format module implements this for its compressor and decompressor beside the real
    /// implementation, so adding a format needs no edit here.
    pub trait Compression {}

    impl<D> Compression for Box<dyn super::Compression<Mode = D>> {}
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
/// # The methods are an internal detail
///
/// What this trait is *for* is naming an operation: `impl Compression<Mode = Compress>` accepts any
/// compressor and no decompressor. Its methods are how this crate drives one, and are documented
/// here only for the reader of this crate's own source. Treat them as internal: they are hidden
/// from the rendered documentation, and they can change without that being a breaking change worth
/// announcing.
///
/// Reach for [`compress`][crate::compress] and [`decompress`][crate::decompress] for a complete
/// buffer, or [`CompressionStream`][crate::CompressionStream] for data that arrives over time.
///
/// # Examples
///
/// ```
/// use bytesbuf::BytesView;
/// use bytesbuf::mem::{GlobalPool, MemoryShared};
/// use compressors::core::{Compress, Compression};
/// use compressors::{Resources, gzip};
///
/// fn compress(compression: impl Compression<Mode = Compress>, input: BytesView) -> compressors::Result<BytesView> {
///     compressors::compress(input, compression)
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
    #[doc(hidden)]
    fn push(&mut self, input: BytesView) -> Result<()>;

    /// Signals that no further input will be supplied.
    #[doc(hidden)]
    fn end_input(&mut self);

    /// Produces the next output chunk.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying engine fails or the input is invalid.
    #[doc(hidden)]
    fn pull(&mut self) -> Result<Output>;

    /// The number of bytes consumed from the input so far.
    #[doc(hidden)]
    fn total_in(&self) -> u64;

    /// The number of bytes produced so far.
    #[doc(hidden)]
    fn total_out(&self) -> u64;

    /// Requests a resumable flush of everything supplied so far.
    ///
    /// Drain [`pull`][Compression::pull] until it reports [`Output::NeedInput`] before pushing more
    /// input. Flushing ends a compressed block early, which can cost compression ratio, so use it
    /// only when the bytes have to reach the far end before the stream does.
    ///
    /// Decompression has nothing to flush -- output is already produced as soon as the input allows
    /// -- so this does nothing there, which is what the default implementation is.
    ///
    /// # Errors
    ///
    /// Returns an invalid-state error after end of input or a previous operation failure.
    #[doc(hidden)]
    fn flush(&mut self) -> Result<()> {
        Ok(())
    }
}

/// Drives one complete input through `operation` and returns the whole result.
///
/// This is [`push`][Compression::push], [`end_input`][Compression::end_input] and draining
/// [`pull`][Compression::pull] in one call. It ends the operation, so an operation serves one call,
/// and it buffers the entire result: drive `pull` directly to stay bounded by the chunk size.
///
/// # Errors
///
/// Returns an error if the underlying engine fails or the input is invalid.
pub(crate) fn process(mut operation: impl Compression, input: BytesView) -> Result<BytesView> {
    operation.push(input)?;
    operation.end_input();

    let mut collected = BytesBuf::new();
    loop {
        match operation.pull()? {
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

    fn flush(&mut self) -> Result<()> {
        (**self).flush()
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

        let result =
            process(ProgressOnceThenDone { done: false }, view(b"ignored")).expect("process succeeds even when a step only makes progress");

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

        let error =
            process(NeedsMoreForever, view(b"ignored")).expect_err("process rejects a pull that still requests input after end of input");
        assert!(error.is_invalid_state());
    }
}
