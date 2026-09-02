// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The brotli codec.
//!
//! Brotli is a genuinely different engine from the deflate family: a different state type, a
//! different way of signaling completion, and an output slice that must already be initialized.
//! It is the format that proves the [`Codec`] abstraction is not just shaped around flate2.

use std::mem::MaybeUninit;

use brotli::enc::StandardAlloc;
use brotli::enc::encode::{BrotliEncoderOperation, BrotliEncoderStateStruct};
use brotli::{BrotliDecompressStream, BrotliResult, BrotliState, HeapAlloc, HuffmanCode};

use crate::brotli::{CompressorOptions, Mode};
use crate::engine::{Codec, Operation, Step, StreamEnd};
use crate::error::{Error, Result};
use crate::level::Level;
use crate::limits::FormatLimits;
use crate::trailing::TrailingData;

/// Brotli's native quality range is `0..=11`, wider than the portable [`Level`] scale of `0..=9`.
///
/// A round-to-nearest linear map, so the endpoints line up (`0 -> 0`, `9 -> 11`) and the mapping
/// stays monotonic.
fn portable_quality(level: Level) -> u32 {
    let scaled = (u32::from(level.get()) * 11 + 4) / 9;
    scaled.min(11)
}

/// Initializes an uninitialized output slice so brotli, which writes into `&mut [u8]`, can use it.
///
/// The deflate backend performs the same zero-fill internally, so this is not extra work relative
/// to the other formats.
fn initialize(output: &mut [MaybeUninit<u8>]) -> &mut [u8] {
    for slot in &mut *output {
        slot.write(0);
    }

    // SAFETY: every element of the slice was just initialized by the loop above, and `u8` has the
    // same layout as `MaybeUninit<u8>`.
    unsafe { &mut *(std::ptr::from_mut(output) as *mut [u8]) }
}

/// `compress_stream` reports failure only when it is driven inconsistently, for example by
/// supplying new input after the encoder has already reached a terminal state. The engine's
/// [`Pump`][crate::engine::Pump] never calls [`Codec::step`] again once a compressor reports
/// [`Step::StreamEnd`], so this crate can never actually trigger it.
#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg_attr(test, mutants::skip)]
#[cold]
fn compress_stream_failed() -> Error {
    Error::invalid_state("the brotli compression engine reported a failure")
}

pub(crate) struct BrotliCompress {
    state: BrotliEncoderStateStruct<StandardAlloc>,
    finished: bool,
    configuration_valid: bool,
}

impl BrotliCompress {
    pub(crate) fn new(level: Level, options: CompressorOptions) -> Self {
        use brotli::enc::encode::BrotliEncoderParameter;

        let mut state = BrotliEncoderStateStruct::new(StandardAlloc::default());
        let quality = options
            .quality
            .map_or_else(|| portable_quality(level), |quality| u32::from(quality.get()));
        let configuration_valid = state.set_parameter(BrotliEncoderParameter::BROTLI_PARAM_QUALITY, quality)
            && state.set_parameter(BrotliEncoderParameter::BROTLI_PARAM_LGWIN, u32::from(options.window_size.get()))
            && state.set_parameter(BrotliEncoderParameter::BROTLI_PARAM_MODE, mode(options.mode));

        Self {
            state,
            finished: false,
            configuration_valid,
        }
    }
}

/// Maps our [`Mode`] onto brotli's numeric parameter.
fn mode(mode: Mode) -> u32 {
    match mode {
        Mode::Generic => 0,
        Mode::Text => 1,
        Mode::Font => 2,
    }
}

impl std::fmt::Debug for BrotliCompress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BrotliCompress")
            .field("finished", &self.finished)
            .finish_non_exhaustive()
    }
}

impl Codec for BrotliCompress {
    fn step(&mut self, input: &[u8], output: &mut [MaybeUninit<u8>], operation: Operation) -> Result<(Step, usize, usize)> {
        if !self.configuration_valid {
            return Err(Error::invalid_configuration(
                "the brotli compression engine rejected its configuration",
            ));
        }

        let brotli_operation = match operation {
            Operation::Process => BrotliEncoderOperation::BROTLI_OPERATION_PROCESS,
            Operation::Flush => BrotliEncoderOperation::BROTLI_OPERATION_FLUSH,
            Operation::Finish => BrotliEncoderOperation::BROTLI_OPERATION_FINISH,
        };

        let out = initialize(output);
        let mut available_in = input.len();
        let mut input_offset = 0_usize;
        let mut available_out = out.len();
        let mut output_offset = 0_usize;
        let mut total_out = None;

        let ok = self.state.compress_stream(
            brotli_operation,
            &mut available_in,
            input,
            &mut input_offset,
            &mut available_out,
            out,
            &mut output_offset,
            &mut total_out,
            &mut |_, _, _, _| (),
        );

        ok.then_some(()).ok_or_else(compress_stream_failed)?;

        self.finished = self.state.is_finished();
        let step = if self.finished {
            Step::StreamEnd
        } else if operation == Operation::Flush && available_in == 0 && !self.state.has_more_output() {
            Step::FlushComplete
        } else {
            Step::Continue
        };

        Ok((step, input_offset, output_offset))
    }

    fn stream_ended(&mut self) -> Result<StreamEnd> {
        Ok(StreamEnd::Complete)
    }
}

pub(crate) struct BrotliDecompress {
    state: BrotliState<HeapAlloc<u8>, HeapAlloc<u32>, HeapAlloc<HuffmanCode>>,
    limits: FormatLimits,
    multi_stream: bool,
    trailing_data: TrailingData,
    needs_reset: bool,
    total_out: usize,
}

impl BrotliDecompress {
    pub(crate) fn new(limits: FormatLimits, multi_stream: bool, trailing_data: TrailingData) -> Self {
        Self {
            state: Self::state(),
            limits,
            multi_stream,
            trailing_data,
            needs_reset: false,
            total_out: 0,
        }
    }

    fn state() -> BrotliState<HeapAlloc<u8>, HeapAlloc<u32>, HeapAlloc<HuffmanCode>> {
        BrotliState::new(HeapAlloc::new(0), HeapAlloc::new(0), HeapAlloc::new(HuffmanCode::default()))
    }
}

impl std::fmt::Debug for BrotliDecompress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BrotliDecompress")
            .field("limits", &self.limits)
            .field("multi_stream", &self.multi_stream)
            .field("trailing_data", &self.trailing_data)
            .finish_non_exhaustive()
    }
}

impl Codec for BrotliDecompress {
    fn step(&mut self, input: &[u8], output: &mut [MaybeUninit<u8>], _operation: Operation) -> Result<(Step, usize, usize)> {
        if self.needs_reset {
            self.state = Self::state();
            self.total_out = 0;
            self.needs_reset = false;
        }

        let out = initialize(output);
        let mut available_in = input.len();
        let mut input_offset = 0_usize;
        let mut available_out = out.len();
        let mut output_offset = 0_usize;

        let result = BrotliDecompressStream(
            &mut available_in,
            &mut input_offset,
            input,
            &mut available_out,
            &mut output_offset,
            out,
            &mut self.total_out,
            &mut self.state,
        );

        let step = match result {
            BrotliResult::ResultSuccess => Step::StreamEnd,
            BrotliResult::NeedsMoreInput | BrotliResult::NeedsMoreOutput => Step::Continue,
            BrotliResult::ResultFailure => {
                return Err(Error::corrupt_data("the compressed data is not a valid brotli stream"));
            }
        };

        Ok((step, input_offset, output_offset))
    }

    fn stream_ended(&mut self) -> Result<StreamEnd> {
        if !self.multi_stream {
            return Ok(match self.trailing_data {
                TrailingData::Preserve => StreamEnd::Complete,
                TrailingData::Reject => StreamEnd::AwaitEof,
            });
        }

        self.needs_reset = true;
        Ok(StreamEnd::NextStream)
    }

    fn check_limits(&self, total_in: u64, total_out: u64, streams: u64) -> Result<()> {
        self.limits.check(total_in, total_out, streams)
    }

    fn remaining_output(&self, total_out: u64) -> Option<u64> {
        self.limits.remaining_output(total_out)
    }

    fn max_streams(&self) -> Option<u64> {
        self.limits.max_streams()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quality_maps_the_portable_scale_onto_brotlis_range() {
        assert_eq!(portable_quality(Level::MIN), 0, "the floor must line up");
        assert_eq!(portable_quality(Level::HIGH), 11, "the ceiling must line up");

        let mut previous = None;
        for raw in 0..=Level::MAX.get() {
            let level = Level::new(raw).expect("level is in range");
            let mapped = portable_quality(level);

            assert!(Some(mapped) > previous, "mapping must be strictly monotonic at level {raw}");
            assert!(mapped <= 11, "level {raw} mapped outside brotli's range");
            previous = Some(mapped);
        }
    }

    #[test]
    fn every_mode_maps_to_its_own_brotli_parameter() {
        assert_eq!(mode(Mode::Generic), 0);
        assert_eq!(mode(Mode::Text), 1);
        assert_eq!(mode(Mode::Font), 2);
    }

    #[test]
    fn initialize_zeroes_the_whole_slice() {
        let mut raw = [MaybeUninit::new(0xff_u8); 8];
        let initialized = initialize(&mut raw);

        assert_eq!(initialized, &[0_u8; 8]);
    }

    #[test]
    fn rejected_configuration_surfaces_on_first_step() {
        let mut codec = BrotliCompress::new(Level::DEFAULT, CompressorOptions::default());
        codec.configuration_valid = false;
        let mut output = [MaybeUninit::uninit(); 8];

        let error = codec
            .step(b"input", &mut output, Operation::Process)
            .expect_err("invalid configuration is reported");
        assert!(error.is_invalid_configuration(), "got {error}");
    }

    #[test]
    fn decompressor_debug_includes_its_policies() {
        let codec = BrotliDecompress::new(FormatLimits::new(None, None), false, TrailingData::Reject);
        let rendered = format!("{codec:?}");

        assert!(rendered.contains("trailing_data"));
        assert!(rendered.contains("Reject"));
    }

    #[test]
    fn remaining_output_delegates_to_the_configured_limits() {
        let codec = BrotliDecompress::new(FormatLimits::new(None, Some(100)), false, TrailingData::Reject);

        assert_eq!(Codec::remaining_output(&codec, 40), Some(60));
        assert_eq!(Codec::remaining_output(&codec, 100), Some(0));
    }

    #[test]
    fn compressor_debug_includes_its_finished_flag() {
        let codec = BrotliCompress::new(Level::DEFAULT, CompressorOptions::default());
        let rendered = format!("{codec:?}");

        assert!(rendered.contains("BrotliCompress"));
        assert!(rendered.contains("finished"));
    }
}
