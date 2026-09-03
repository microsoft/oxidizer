// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Binds zstd to the engine, driven segment by segment.

use std::mem::MaybeUninit;

use zstd_safe::zstd_sys::ZSTD_EndDirective;
use zstd_safe::{CCtx, CParameter, DCtx, DParameter, InBuffer, OutBuffer, ResetDirective};

use crate::engine::{Codec, Operation, Step, StreamEnd};
use crate::error::{BuildError, Error, Result};
use crate::level::Level;
use crate::limits::FormatLimits;
use crate::pool::Pool;
use crate::trailing::TrailingData;
use crate::zstd::{CompressionLevel, Zstd};

/// Maps the portable [`Level`] scale onto zstd's levels.
///
/// zstd accepts 1 to 22, but the top of that range is not a sensible destination for a portable
/// "highest quality" setting: measured on realistic JSON, level 19 is over 200 times slower than
/// level 3 for about `17%` better compression, and 22 buys nothing over 19 at all. The scale is
/// therefore anchored on zstd's own default rather than stretched across the whole range, so
/// [`Level::DEFAULT`] means what it says on every format -- a balanced trade-off.
///
/// Reach the levels above this range with
/// [`CompressorBuilder::compression_level`][crate::zstd::CompressorBuilder::compression_level].
fn compression_level(level: Level) -> i32 {
    // 0..=6 spans zstd 1..=3 (its default); 7..=9 climbs to 12, past which cost explodes.
    const MAPPING: [i32; 10] = [1, 1, 2, 2, 3, 3, 3, 6, 9, 12];

    MAPPING[usize::from(level.get().min(9))]
}

/// Lends zstd an output slice that is still uninitialized.
///
/// Zstd writes into the output without ever reading it, and `WriteBuf` is the trait zstd-safe
/// provides to say exactly that: the capacity may be uninitialized, and the callee reports what it
/// filled. Handing over the engine's spare capacity directly is what keeps this codec from zeroing
/// a whole output chunk before every step, which would defeat the point of reserving uninitialized
/// memory in the first place.
struct UninitOutput<'a> {
    buffer: &'a mut [MaybeUninit<u8>],
    /// How many bytes from the front zstd has reported writing.
    filled: usize,
}

// SAFETY: `as_mut_ptr` returns a pointer to `capacity` writable bytes that stays valid for the
// borrow, and `as_slice` never covers more than `filled`, which only ever advances through
// `filled_until` -- whose own contract is that the caller initialized that many bytes, and which
// clamps to `capacity` so `filled` can never exceed the allocation.
unsafe impl zstd_safe::WriteBuf for UninitOutput<'_> {
    fn as_slice(&self) -> &[u8] {
        // SAFETY: `filled_until` promised these bytes are initialized, and `u8` shares its layout
        // with `MaybeUninit<u8>`.
        unsafe { std::slice::from_raw_parts(self.buffer.as_ptr().cast::<u8>(), self.filled) }
    }

    fn capacity(&self) -> usize {
        self.buffer.len()
    }

    fn as_mut_ptr(&mut self) -> *mut u8 {
        self.buffer.as_mut_ptr().cast::<u8>()
    }

    // zstd is handed `capacity()` and reports back how much of it it wrote, so `n` is always within
    // the buffer. The clamp holds `as_slice` sound by construction rather than by trusting the
    // binding to honour that: were it ever violated, the out-of-bounds write would already have
    // happened, and this at least stops it becoming a lasting out-of-bounds read.
    unsafe fn filled_until(&mut self, n: usize) {
        debug_assert!(n <= self.buffer.len(), "zstd reported writing more than the capacity it was given");
        self.filled = n.min(self.buffer.len());
    }
}

impl<'a> UninitOutput<'a> {
    fn new(buffer: &'a mut [MaybeUninit<u8>]) -> Self {
        Self { buffer, filled: 0 }
    }
}

/// Reads zstd's "bytes still buffered" answer as a step outcome.
///
/// `remaining == 0` is how zstd says the epilogue is out, so treating it as anything else leaves a
/// finish that never completes: the mutant hangs rather than failing, and the harness records a
/// timeout instead of a verdict.
#[cfg_attr(test, mutants::skip)]
fn finish_step(operation: Operation, remaining: usize) -> Step {
    match operation {
        Operation::Finish if remaining == 0 => Step::StreamEnd,
        Operation::Flush if remaining == 0 => Step::FlushComplete,
        _ => Step::Continue,
    }
}

fn compression_failed(code: usize) -> Error {
    Error::invalid_state(format!("zstd compression failed: {}", zstd_safe::get_error_name(code)))
}

fn decompression_failed(code: usize) -> Error {
    Error::corrupt_data(format!("zstd decompression failed: {}", zstd_safe::get_error_name(code)))
}

/// zstd only rejects a compression level outside its own `min_c_level()..=max_c_level()` range.
/// [`CompressionLevel::new`][crate::zstd::CompressionLevel::new] and [`compression_level`] both
/// stay inside exactly that range, so `set_parameter` can never actually reject the level this
/// crate passes in.
#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg_attr(test, mutants::skip)]
#[cold]
fn compression_level_rejected(level: i32, code: usize) -> BuildError {
    BuildError::new(format!(
        "zstd rejected compression level {level}: {}",
        zstd_safe::get_error_name(code)
    ))
}

/// zstd clamps `WindowLogMax` to `ZSTD_WINDOWLOG_MIN..=ZSTD_WINDOWLOG_MAX`, and
/// [`WindowLog`][crate::zstd::WindowLog]'s own bounds are defined as exactly that range, so
/// `set_parameter` can never actually reject a window log this crate passes in.
#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg_attr(test, mutants::skip)]
#[cold]
fn window_log_rejected(window: u32, code: usize) -> BuildError {
    BuildError::new(format!(
        "zstd rejected maximum window log {window}: {}",
        zstd_safe::get_error_name(code)
    ))
}

/// Resetting a session, with no parameters to validate, has no documented failure mode; this
/// guards against a native error the bundled zstd version has never been observed to return.
#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg_attr(test, mutants::skip)]
#[cold]
fn reset_for_next_frame_failed(code: usize) -> Error {
    Error::invalid_state(format!(
        "zstd failed to reset for the next frame: {}",
        zstd_safe::get_error_name(code)
    ))
}

pub(crate) struct ZstdCompress {
    /// `Some` until the context is handed back in `drop`.
    context: Option<CCtx<'static>>,
    level: i32,
    recycle: Pool,
}

impl ZstdCompress {
    pub(crate) fn new(level: Level, options: &Zstd, pool: Pool) -> ::core::result::Result<Self, BuildError> {
        let level = options.level.map_or_else(|| compression_level(level), CompressionLevel::get);
        let mut context = pool.take_zstd_compressor(level).unwrap_or_else(CCtx::create);

        // Applied unconditionally: a recycled context comes back with its parameters cleared, so
        // that a recycled compressor is indistinguishable from a fresh one.
        context
            .set_parameter(CParameter::CompressionLevel(level))
            .map_err(|code| compression_level_rejected(level, code))?;

        Ok(Self {
            context: Some(context),
            level,
            recycle: pool,
        })
    }

    fn engine(&mut self) -> &mut CCtx<'static> {
        self.context.as_mut().expect("the context is only taken in drop")
    }
}

impl std::fmt::Debug for ZstdCompress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ZstdCompress").field("level", &self.level).finish_non_exhaustive()
    }
}

impl Drop for ZstdCompress {
    fn drop(&mut self) {
        self.recycle.return_zstd_compressor(self.level, &mut self.context);
    }
}

// SAFETY: `step` writes through `zstd_safe::WriteBuf`, which takes the uninitialized slice and
// reports what zstd filled, so the count is the engine's own.
unsafe impl Codec for ZstdCompress {
    fn step(&mut self, input: &[u8], output: &mut [MaybeUninit<u8>], operation: Operation) -> Result<(Step, usize, usize)> {
        let directive = match operation {
            Operation::Process => ZSTD_EndDirective::ZSTD_e_continue,
            Operation::Flush => ZSTD_EndDirective::ZSTD_e_flush,
            Operation::Finish => ZSTD_EndDirective::ZSTD_e_end,
        };

        let mut out = UninitOutput::new(output);
        let mut in_buffer = InBuffer::around(input);
        let mut out_buffer = OutBuffer::around(&mut out);

        let remaining = self
            .engine()
            .compress_stream2(&mut out_buffer, &mut in_buffer, directive)
            .map_err(compression_failed)?;

        // `ZSTD_e_end` reports zero only once the frame's epilogue has been flushed.
        let step = finish_step(operation, remaining);

        Ok((step, in_buffer.pos(), out_buffer.pos()))
    }

    fn stream_ended(&mut self) -> Result<StreamEnd> {
        Ok(StreamEnd::Complete)
    }
}

pub(crate) struct ZstdDecompress {
    /// `Some` until the context is handed back in `drop`.
    context: Option<DCtx<'static>>,
    limits: FormatLimits,
    multi_stream: bool,
    trailing_data: TrailingData,
    recycle: Pool,
    needs_reset: bool,
}

impl ZstdDecompress {
    pub(crate) fn new(
        limits: FormatLimits,
        multi_stream: bool,
        trailing_data: TrailingData,
        options: &Zstd,
        pool: Pool,
    ) -> ::core::result::Result<Self, BuildError> {
        let mut context = pool.take_zstd_decompressor().unwrap_or_else(DCtx::create);

        if let Some(window) = options.max_window_log {
            context
                .set_parameter(DParameter::WindowLogMax(window.get()))
                .map_err(|code| window_log_rejected(window.get(), code))?;
        }

        Ok(Self {
            context: Some(context),
            limits,
            multi_stream,
            trailing_data,
            recycle: pool,
            needs_reset: false,
        })
    }

    fn engine(&mut self) -> &mut DCtx<'static> {
        self.context.as_mut().expect("the context is only taken in drop")
    }
}

impl std::fmt::Debug for ZstdDecompress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ZstdDecompress")
            .field("limits", &self.limits)
            .field("multi_stream", &self.multi_stream)
            .field("trailing_data", &self.trailing_data)
            .finish_non_exhaustive()
    }
}

impl Drop for ZstdDecompress {
    fn drop(&mut self) {
        self.recycle.return_zstd_decompressor(&mut self.context);
    }
}

// SAFETY: `step` writes through `zstd_safe::WriteBuf`, which takes the uninitialized slice and
// reports what zstd filled, so the count is the engine's own.
unsafe impl Codec for ZstdDecompress {
    fn step(&mut self, input: &[u8], output: &mut [MaybeUninit<u8>], _operation: Operation) -> Result<(Step, usize, usize)> {
        if self.needs_reset {
            self.engine()
                .reset(ResetDirective::SessionOnly)
                .map_err(reset_for_next_frame_failed)?;
            self.needs_reset = false;
        }

        let mut out = UninitOutput::new(output);
        let mut in_buffer = InBuffer::around(input);
        let mut out_buffer = OutBuffer::around(&mut out);

        let hint = self
            .engine()
            .decompress_stream(&mut out_buffer, &mut in_buffer)
            .map_err(decompression_failed)?;

        // Zero means the frame ended exactly here; anything else is a hint at the next read size.
        let step = if hint == 0 { Step::StreamEnd } else { Step::Continue };

        Ok((step, in_buffer.pos(), out_buffer.pos()))
    }

    fn stream_ended(&mut self) -> Result<StreamEnd> {
        if !self.multi_stream {
            return Ok(match self.trailing_data {
                TrailingData::Ignore => StreamEnd::Complete,
                TrailingData::Reject => StreamEnd::AwaitEof,
            });
        }

        // Reset only if another frame actually arrives, so the common single-frame path does no
        // terminal cleanup work.
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

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_level_mapping_is_monotonic_and_within_zstds_range() {
        let mut previous = 0;
        for raw in 0..=Level::MAX.get() {
            let level = Level::new(raw).expect("level is in range");
            let mapped = compression_level(level);

            assert!(mapped >= previous, "mapping must not decrease at level {raw}");
            assert!((1..=22).contains(&mapped), "level {raw} mapped outside zstd's range");
            previous = mapped;
        }
    }

    #[test]
    fn the_default_level_maps_to_zstds_own_default() {
        // The whole point of anchoring rather than stretching: `Level::DEFAULT` must mean
        // "balanced" on every format, and zstd's balanced point is 3, not the middle of 1..=22.
        assert_eq!(compression_level(Level::DEFAULT), 3);
        assert_eq!(compression_level(Level::MIN), 1);
        assert_eq!(compression_level(Level::HIGH), 12);
    }

    #[test]
    fn the_uninit_output_only_exposes_what_zstd_reported_writing() {
        // The adapter's whole job is to hand out uninitialized capacity while never letting anyone
        // read past the prefix zstd said it filled.
        let mut raw = [MaybeUninit::new(0xff_u8); 8];
        let mut out = UninitOutput::new(&mut raw);

        assert_eq!(zstd_safe::WriteBuf::capacity(&out), 8);
        assert!(zstd_safe::WriteBuf::as_slice(&out).is_empty(), "nothing is initialized yet");

        // SAFETY: the eight bytes were initialized when `raw` was built.
        unsafe { zstd_safe::WriteBuf::filled_until(&mut out, 3) };

        assert_eq!(zstd_safe::WriteBuf::as_slice(&out), &[0xff_u8; 3]);
    }

    #[test]
    fn native_error_helpers_keep_compression_and_decompression_distinct() {
        assert!(compression_failed(0).is_invalid_state());
        assert!(decompression_failed(0).is_corrupt_data());
    }

    #[test]
    fn every_expressible_configuration_is_accepted_by_the_engine() {
        use crate::zstd::WindowLog;

        let mut settings = Zstd::new();
        for level in [CompressionLevel::min(), CompressionLevel::DEFAULT, CompressionLevel::max()] {
            settings.level = Some(level);
            ZstdCompress::new(Level::DEFAULT, &settings, Pool::disabled().clone()).expect("the engine accepts every native level");
        }

        for log in [WindowLog::MIN, WindowLog::DEFAULT, WindowLog::MAX] {
            let mut settings = Zstd::new();
            settings.max_window_log = Some(log);
            ZstdDecompress::new(
                FormatLimits::new(None, None, None),
                false,
                TrailingData::Reject,
                &settings,
                Pool::disabled().clone(),
            )
            .expect("the engine accepts every window log the builder can express");
        }
    }

    #[test]
    fn decompressor_debug_includes_its_policies() {
        let codec = ZstdDecompress::new(
            FormatLimits::new(None, None, None),
            false,
            TrailingData::Reject,
            &Zstd::new(),
            Pool::disabled().clone(),
        )
        .expect("the default settings are accepted");
        let rendered = format!("{codec:?}");

        assert!(rendered.contains("trailing_data"));
        assert!(rendered.contains("Reject"));
    }

    #[test]
    fn compressor_debug_includes_its_level() {
        let codec = ZstdCompress::new(Level::DEFAULT, &Zstd::new(), Pool::disabled().clone()).expect("the default settings are accepted");
        let rendered = format!("{codec:?}");

        assert!(rendered.contains("ZstdCompress"));
        assert!(rendered.contains("level"));
    }

    #[test]
    fn dropping_a_pooled_compressor_returns_its_context() {
        let pool = Pool::new();
        let level = compression_level(Level::DEFAULT);

        drop(ZstdCompress::new(Level::DEFAULT, &Zstd::new(), pool.clone()).expect("the default settings are accepted"));

        assert!(
            pool.take_zstd_compressor(level).is_some(),
            "the context should have been returned to the pool"
        );
    }

    #[test]
    fn dropping_a_pooled_decompressor_returns_its_context() {
        let pool = Pool::new();

        drop(
            ZstdDecompress::new(
                FormatLimits::new(None, None, None),
                false,
                TrailingData::Reject,
                &Zstd::new(),
                pool.clone(),
            )
            .expect("the default settings are accepted"),
        );

        assert!(
            pool.take_zstd_decompressor().is_some(),
            "the context should have been returned to the pool"
        );
    }

    #[test]
    fn a_flush_reports_continue_until_the_native_buffer_catches_up() {
        let mut codec =
            ZstdCompress::new(Level::DEFAULT, &Zstd::new(), Pool::disabled().clone()).expect("the default settings are accepted");
        let mut scratch = [MaybeUninit::uninit(); 4096];

        let payload = b"zstd flush boundary check payload, repeated so the flush has real work to do. ".repeat(64);
        let (_, consumed, _) = codec.step(&payload, &mut scratch, Operation::Process).expect("process succeeds");
        assert_eq!(consumed, payload.len(), "the whole input should have been consumed");

        // A one byte buffer cannot hold the whole flush in a single call, so the guard must
        // report `Continue`, not `FlushComplete`, while zstd still has buffered output.
        let mut tiny = [MaybeUninit::uninit(); 1];
        let (step, consumed, produced) = codec.step(&[], &mut tiny, Operation::Flush).expect("flush succeeds");
        assert_eq!(consumed, 0, "no new input was supplied");
        assert_eq!(produced, 1, "the tiny buffer should be filled completely");
        assert_eq!(step, Step::Continue, "the flush cannot be complete while output remains buffered");

        // A generous buffer drains the rest of the same flush and reports completion. This must
        // be a single call, not a retry loop: calling `Flush` again after it already completed
        // would ask zstd to emit another empty flush frame, so the test only issues exactly the
        // calls this one flush needs.
        let mut generous = [MaybeUninit::uninit(); 4096];
        let (step, consumed, _) = codec.step(&[], &mut generous, Operation::Flush).expect("flush succeeds");
        assert_eq!(consumed, 0, "no new input was supplied");
        assert_eq!(step, Step::FlushComplete, "a generous buffer must drain the remainder of the flush");
    }

    #[test]
    fn remaining_output_delegates_to_the_configured_limits() {
        let codec = ZstdDecompress::new(
            FormatLimits::new(None, Some(100), None),
            false,
            TrailingData::Reject,
            &Zstd::new(),
            Pool::disabled().clone(),
        )
        .expect("the default settings are accepted");

        assert_eq!(Codec::remaining_output(&codec, 40), Some(60));
        assert_eq!(Codec::remaining_output(&codec, 100), Some(0));
    }
}
