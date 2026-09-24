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

mod destination;
mod output;

pub(crate) use destination::Destination;
pub(crate) use output::Output;

pub(crate) mod internal {
    use std::fmt;

    use bytesbuf::BytesView;

    use super::{Destination, Output};
    use crate::error::Result;

    /// The push/pull mechanics behind every [`Compression`][super::Compression] implementation.
    ///
    /// This is deliberately kept off [`Compression`][super::Compression], and this module is
    /// `pub(crate)`, so none of it reaches the public API. What that trait is for is *naming* an
    /// engine; these methods are how this crate drives one.
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
        /// `into` says what the caller will do with it, and the engine bounds its own output
        /// accordingly: a decompressor left unbounded applies the shared 64 MiB ceiling for
        /// [`Destination::Buffer`], because that is the case where its cumulative output is what
        /// the caller retains. Compressors ignore it -- compressed output tracks input the caller
        /// already holds.
        ///
        /// # Errors
        ///
        /// Returns an error if the underlying engine fails, the input is invalid, or a bound that
        /// applies to this destination has been reached.
        fn pull(&mut self, into: Destination) -> Result<Output>;

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
        /// Returns an invalid-state error after end of input or a previous failure.
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

/// A streaming compression or decompression engine.
///
/// Every format's compressor and decompressor implements this contract. The `Mode` associated type
/// records which direction an implementation compresses in without changing how callers drive it. This
/// allows shared processing code to accept any `Compression`, while APIs that require one direction
/// can use `Compression<Mode = Compress>` or `Compression<Mode = Decompress>`.
///
/// The trait is sealed so formats and methods can be added without breaking downstream code, and
/// it is `Sized` so no `dyn Compression` object can be formed. Every implementation is
/// `Send + Sync`.
///
/// # The mechanics are an internal detail
///
/// What this trait is *for* is naming an engine: `impl Compression<Mode = Compress>` accepts any
/// compressor and no decompressor. How this crate actually drives one -- pushing input, pulling
/// output, ending input -- lives on a crate-private supertrait that no downstream crate can name,
/// let alone implement. Those mechanics are therefore not public API and can change freely.
///
/// The `Sized` bound is what makes that true rather than merely intended. A trait object resolves
/// supertrait methods as inherent candidates, with no need for the supertrait to be nameable or in
/// scope, so a `dyn Compression` vtable would hand every downstream crate the push/pull mechanics
/// and values of the crate-private step type. Boxing a concrete compressor stays available and is
/// how to hold one without naming its type.
///
/// Reach for [`compress`][crate::compress] and [`decompress`][crate::decompress] for a complete
/// buffer, or `CompressionStream` (behind the `futures-stream` feature) for data that arrives over time.
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
pub trait Compression: CompressionInternal + Sized {
    /// Whether this implementation compresses or decompresses its input.
    type Mode;
}

/// Drives one complete input through `engine` and returns the whole result.
///
/// This is [`push`][Compression::push], [`end_input`][Compression::end_input] and draining
/// [`pull`][Compression::pull] in one call. It ends the engine, so an engine serves one call,
/// and it buffers the entire result: drive `pull` directly to stay bounded by the chunk size.
///
/// Every `pull` here says [`Destination::Buffer`], which is what tells a decompressor that its
/// cumulative output is what the caller will retain. The engine applies its own ceiling on that
/// basis, so this loop does no bounding of its own.
///
/// # Errors
///
/// Returns an error if the underlying engine fails, the input is invalid, or the engine's bounds
/// for a buffered destination are reached.
pub(crate) fn process(mut engine: impl Compression, input: BytesView) -> Result<BytesView> {
    engine.push(input)?;
    engine.end_input();

    let mut collected = BytesBuf::new();
    loop {
        match engine.pull(Destination::Buffer)? {
            Output::Data(chunk) => collected.put_bytes(chunk),
            Output::Progress => {}
            Output::Done => break,
            Output::NeedInput => {
                return Err(crate::Error::invalid_state("the engine requested input after end of input"));
            }
        }
    }

    Ok(collected.consume_all())
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::view;

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

            fn pull(&mut self, _into: Destination) -> Result<Output> {
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

        let result = process(ProgressOnceThenDone { done: false }, view(b"ignored")).unwrap();

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

            fn pull(&mut self, _into: Destination) -> Result<Output> {
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

        let error = process(NeedsMoreForever, view(b"ignored")).unwrap_err();
        assert!(error.is_invalid_state());
    }
}
