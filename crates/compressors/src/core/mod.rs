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
//! What one step of that contract reports is a crate-private detail, as are the methods that drive
//! it: this module publishes the names an API is written against, and nothing else.

use bytesbuf::{BytesBuf, BytesView};

use crate::error::Result;

mod output;

pub(crate) use output::Output;

pub(crate) mod internal {
    use std::fmt;

    use bytesbuf::BytesView;

    use super::Output;
    use crate::error::Result;

    /// The push/pull mechanics behind every [`Compression`][super::Compression] implementation.
    ///
    /// This is deliberately kept off [`Compression`][super::Compression], and this module is
    /// `pub(crate)`, so none of it reaches the public API. What that trait is for is *naming* an
    /// operation; these methods are how this crate drives one.
    ///
    /// Being unnameable outside the crate is also what seals [`Compression`][super::Compression]:
    /// a downstream crate cannot implement a supertrait it cannot refer to, so formats and methods
    /// can be added here without breaking anyone. Every implementation is `Send + Sync`.
    pub trait CompressionInternal: fmt::Debug + Send + Sync {
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

        /// Requests a resumable flush of everything supplied so far.
        ///
        /// Drain [`pull`][CompressionInternal::pull] until it reports [`Output::NeedInput`] before
        /// pushing more input. Flushing ends a compressed block early, which can cost compression
        /// ratio, so use it only when the bytes have to reach the far end before the stream does.
        ///
        /// Decompression has nothing to flush -- output is already produced as soon as the input
        /// allows -- so this does nothing there, which is what the default implementation is.
        ///
        /// # Errors
        ///
        /// Returns an invalid-state error after end of input or a previous operation failure.
        fn flush(&mut self) -> Result<()> {
            Ok(())
        }
    }
}

pub(crate) use internal::CompressionInternal;

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
/// # The mechanics are an internal detail
///
/// What this trait is *for* is naming an operation: `impl Compression<Mode = Compress>` accepts any
/// compressor and no decompressor. How this crate actually drives one -- pushing input, pulling
/// output, ending input -- lives on a crate-private supertrait that no downstream crate can name,
/// let alone implement. Those mechanics are therefore not public API and can change freely.
///
/// Reach for [`compress`][crate::compress] and [`decompress`][crate::decompress] for a complete
/// buffer, or [`CompressionStream`][crate::CompressionStream] for data that arrives over time.
///
/// # Examples
///
/// ```
/// # #[cfg(feature = "gzip")]
/// # {
/// use bytesbuf::BytesView;
/// use bytesbuf::mem::GlobalPool;
/// use compressors::core::{Compress, Compression};
/// use compressors::{Resources, gzip};
///
/// fn compress(
///     compression: impl Compression<Mode = Compress>,
///     input: BytesView,
/// ) -> compressors::Result<BytesView> {
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
/// # }
/// # Ok::<(), compressors::Error>(())
/// ```
pub trait Compression: CompressionInternal {
    /// Whether this implementation compresses or decompresses its input.
    type Mode;
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

/// A fixture that only ever reports progress, for exercising callers that must keep polling rather
/// than treat a progress step as output.
#[cfg(all(test, feature = "futures-stream", feature = "gzip"))]
#[derive(Debug)]
pub(crate) struct ProgressCompression {
    pulls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

#[cfg(all(test, feature = "futures-stream", feature = "gzip"))]
impl ProgressCompression {
    pub(crate) fn new(pulls: std::sync::Arc<std::sync::atomic::AtomicUsize>) -> Self {
        Self { pulls }
    }
}

#[cfg(all(test, feature = "futures-stream", feature = "gzip"))]
impl Compression for ProgressCompression {
    type Mode = Compress;
}

#[cfg(all(test, feature = "futures-stream", feature = "gzip"))]
impl CompressionInternal for ProgressCompression {
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
    #[cfg_attr(test, mutants::skip)]
    fn total_in(&self) -> u64 {
        0
    }

    #[cfg_attr(coverage_nightly, coverage(off))]
    #[cfg_attr(test, mutants::skip)]
    fn total_out(&self) -> u64 {
        0
    }
}

/// A fixture that always asks for input and always rejects it, for exercising callers that must
/// propagate a `push` failure rather than the specific reasons a real codec's `push` can fail.
#[cfg(all(test, feature = "futures-stream", feature = "gzip"))]
#[derive(Debug)]
pub(crate) struct RejectsPush;

#[cfg(all(test, feature = "futures-stream", feature = "gzip"))]
impl Compression for RejectsPush {
    type Mode = Compress;
}

#[cfg(all(test, feature = "futures-stream", feature = "gzip"))]
impl CompressionInternal for RejectsPush {
    // Accepting input would make this fixture, whose whole purpose is to reject it, ask for input
    // endlessly instead. The mutant hangs rather than failing, so no verdict is available.
    #[cfg_attr(test, mutants::skip)]
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
    #[cfg_attr(test, mutants::skip)]
    fn total_in(&self) -> u64 {
        0
    }

    #[cfg_attr(coverage_nightly, coverage(off))]
    #[cfg_attr(test, mutants::skip)]
    fn total_out(&self) -> u64 {
        0
    }
}
#[cfg_attr(coverage_nightly, coverage(off))]
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
        impl Compression for ProgressOnceThenDone {
            type Mode = Compress;
        }

        impl CompressionInternal for ProgressOnceThenDone {
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
            #[cfg_attr(test, mutants::skip)]
            fn total_in(&self) -> u64 {
                0
            }

            #[cfg_attr(coverage_nightly, coverage(off))]
            #[cfg_attr(test, mutants::skip)]
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
        impl Compression for NeedsMoreForever {
            type Mode = Compress;
        }

        impl CompressionInternal for NeedsMoreForever {
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
            #[cfg_attr(test, mutants::skip)]
            fn total_in(&self) -> u64 {
                0
            }

            #[cfg_attr(coverage_nightly, coverage(off))]
            #[cfg_attr(test, mutants::skip)]
            fn total_out(&self) -> u64 {
                0
            }
        }

        let error =
            process(NeedsMoreForever, view(b"ignored")).expect_err("process rejects a pull that still requests input after end of input");
        assert!(error.is_invalid_state());
    }
}
