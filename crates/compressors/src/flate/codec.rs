// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Binds the deflate family to the engine, driven segment by segment.

use std::mem::MaybeUninit;

use flate2::{Compress, Decompress, FlushCompress, FlushDecompress, Status};

use crate::engine::{Codec, Operation, Step, StreamEnd};
use crate::error::{Error, Result};
use crate::flate::Wrapper;
use crate::level::Level;
use crate::limits::FormatLimits;
use crate::pool::{EngineKey, Pool};
use crate::trailing::TrailingData;

#[derive(Debug)]
pub(crate) struct FlateCompress {
    /// `Some` until the engine is handed back in `drop`.
    compress: Option<Compress>,
    recycle: Pool,
    key: EngineKey,
}

impl FlateCompress {
    pub(crate) fn new(wrapper: Wrapper, level: Level, pool: Pool) -> Self {
        let key = EngineKey {
            wrapper,
            level: level.get(),
        };
        let compress = pool.take_compressor(key).unwrap_or_else(|| wrapper.compressor(level));

        Self {
            compress: Some(compress),
            recycle: pool,
            key,
        }
    }

    fn engine(&mut self) -> &mut Compress {
        self.compress.as_mut().expect("the engine is only taken in drop")
    }
}

impl Drop for FlateCompress {
    fn drop(&mut self) {
        self.recycle.return_compressor(self.key, &mut self.compress);
    }
}

impl Codec for FlateCompress {
    fn step(&mut self, input: &[u8], output: &mut [MaybeUninit<u8>], operation: Operation) -> Result<(Step, usize, usize)> {
        let flush = match operation {
            Operation::Process => FlushCompress::None,
            Operation::Flush => FlushCompress::Sync,
            Operation::Finish => FlushCompress::Finish,
        };

        let compress = self.engine();
        let before_in = compress.total_in();
        let before_out = compress.total_out();

        let status = compress
            .compress_uninit(input, output, flush)
            .map_err(|error| Error::invalid_state("the compression engine reported a failure").with_source(error))?;

        let consumed = usize::try_from(compress.total_in() - before_in).unwrap_or(usize::MAX);
        let produced = usize::try_from(compress.total_out() - before_out).unwrap_or(usize::MAX);

        let step = match operation {
            _ if status == Status::StreamEnd => Step::StreamEnd,
            Operation::Flush if consumed == input.len() && produced < output.len() => Step::FlushComplete,
            _ => Step::Continue,
        };

        Ok((step, consumed, produced))
    }

    fn stream_ended(&mut self) -> Result<StreamEnd> {
        Ok(StreamEnd::Complete)
    }
}

#[derive(Debug)]
pub(crate) struct FlateDecompress {
    /// `Some` until the engine is handed back in `drop`.
    decompress: Option<Decompress>,
    wrapper: Wrapper,
    limits: FormatLimits,
    multi_stream: bool,
    trailing_data: TrailingData,
    needs_reset: bool,
    /// Only present where some container's decompressor can actually be recycled.
    #[cfg(any(feature = "deflate", feature = "zlib"))]
    recycle: Pool,
}

impl FlateDecompress {
    pub(crate) fn new(wrapper: Wrapper, limits: FormatLimits, multi_stream: bool, trailing_data: TrailingData, pool: Pool) -> Self {
        // Only containers whose reset restores their framing can be recycled.
        let pool = if wrapper.reset_restores_framing() {
            pool
        } else {
            Pool::disabled().clone()
        };
        let decompress = Self::checkout(wrapper, &pool);

        #[cfg(not(any(feature = "deflate", feature = "zlib")))]
        drop(pool);

        Self {
            decompress: Some(decompress),
            wrapper,
            limits,
            multi_stream,
            trailing_data,
            needs_reset: false,
            #[cfg(any(feature = "deflate", feature = "zlib"))]
            recycle: pool,
        }
    }

    fn checkout(wrapper: Wrapper, pool: &Pool) -> Decompress {
        #[cfg(any(feature = "deflate", feature = "zlib"))]
        if let Some(engine) = pool.take_decompressor(wrapper) {
            return engine;
        }

        let _ = pool;
        wrapper.decompressor()
    }

    fn engine(&mut self) -> &mut Decompress {
        self.decompress.as_mut().expect("the engine is only taken in drop")
    }
}

#[cfg(any(feature = "deflate", feature = "zlib"))]
impl Drop for FlateDecompress {
    fn drop(&mut self) {
        self.recycle.return_decompressor(self.wrapper, &mut self.decompress);
    }
}

impl Codec for FlateDecompress {
    fn step(&mut self, input: &[u8], output: &mut [MaybeUninit<u8>], _operation: Operation) -> Result<(Step, usize, usize)> {
        if self.needs_reset {
            match self.wrapper {
                #[cfg(feature = "deflate")]
                Wrapper::Raw => self.engine().reset(false),
                #[cfg(feature = "zlib")]
                Wrapper::Zlib => self.engine().reset(true),
                #[cfg(feature = "gzip")]
                Wrapper::Gzip => {
                    // `Decompress::reset` cannot express gzip framing.
                    self.decompress = Some(self.wrapper.decompressor());
                }
            }
            self.needs_reset = false;
        }

        let wrapper = self.wrapper;
        let decompress = self.engine();
        let before_in = decompress.total_in();
        let before_out = decompress.total_out();

        let status = decompress
            .decompress_uninit(input, output, FlushDecompress::None)
            .map_err(|error| {
                Error::corrupt_data(format!("the compressed data is not a valid {} stream", wrapper.name())).with_source(error)
            })?;

        let consumed = usize::try_from(decompress.total_in() - before_in).unwrap_or(usize::MAX);
        let produced = usize::try_from(decompress.total_out() - before_out).unwrap_or(usize::MAX);

        let step = if status == Status::StreamEnd {
            Step::StreamEnd
        } else {
            Step::Continue
        };

        Ok((step, consumed, produced))
    }

    fn stream_ended(&mut self) -> Result<StreamEnd> {
        if !self.multi_stream {
            return Ok(match self.trailing_data {
                TrailingData::Ignore => StreamEnd::Complete,
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

#[cfg(all(test, feature = "deflate", feature = "gzip", feature = "zlib"))]
mod tests {
    use super::*;

    #[test]
    fn wrappers_produce_distinguishable_headers() {
        // Guards the framing: a zlib stream must not be mistaken for a gzip one, and raw deflate
        // must carry no header at all.
        let mut headers = Vec::new();

        for wrapper in [Wrapper::Raw, Wrapper::Zlib, Wrapper::Gzip] {
            let mut codec = FlateCompress::new(wrapper, Level::DEFAULT, Pool::disabled().clone());
            let mut out = [MaybeUninit::uninit(); 64];
            let (_, _, produced) = codec
                .step(b"header check", &mut out, Operation::Finish)
                .expect("compression succeeds");

            // SAFETY: the engine reported initializing `produced` bytes.
            let bytes = unsafe { std::slice::from_raw_parts(out.as_ptr().cast::<u8>(), produced) };
            headers.push(bytes[..2].to_vec());
        }

        assert_eq!(headers[2], vec![0x1f, 0x8b], "gzip must carry its magic bytes");
        assert_ne!(headers[0], headers[1], "raw deflate and zlib must differ");
        assert_ne!(headers[1], headers[2], "zlib and gzip must differ");
    }

    #[test]
    fn dropping_a_pooled_compressor_returns_its_engine() {
        let pool = Pool::new();
        let key = EngineKey {
            wrapper: Wrapper::Gzip,
            level: Level::DEFAULT.get(),
        };

        drop(FlateCompress::new(Wrapper::Gzip, Level::DEFAULT, pool.clone()));

        assert!(
            pool.take_compressor(key).is_some(),
            "the engine should have been returned to the pool"
        );
    }

    #[test]
    fn dropping_a_pooled_decompressor_returns_its_engine() {
        let pool = Pool::new();

        drop(FlateDecompress::new(
            Wrapper::Zlib,
            FormatLimits::new(None, None),
            false,
            TrailingData::Reject,
            pool.clone(),
        ));

        assert!(
            pool.take_decompressor(Wrapper::Zlib).is_some(),
            "the engine should have been returned to the pool"
        );
    }

    #[test]
    fn a_flush_reports_continue_until_a_small_output_buffer_catches_up() {
        let mut codec = FlateCompress::new(Wrapper::Gzip, Level::DEFAULT, Pool::disabled().clone());
        let mut scratch = [MaybeUninit::uninit(); 4096];

        let payload = b"flush boundary check payload";
        let (_, consumed, _) = codec.step(payload, &mut scratch, Operation::Process).expect("process succeeds");
        assert_eq!(consumed, payload.len(), "the whole input should have been consumed");

        // A one byte buffer cannot hold the whole flush in a single call, so the guard must
        // report `Continue`, not `FlushComplete`, while output remains buffered.
        let mut tiny = [MaybeUninit::uninit(); 1];
        let (step, consumed, produced) = codec.step(&[], &mut tiny, Operation::Flush).expect("flush succeeds");
        assert_eq!(consumed, 0, "no new input was supplied");
        assert_eq!(produced, 1, "the tiny buffer should be filled completely");
        assert_eq!(step, Step::Continue, "the flush cannot be complete while output remains buffered");

        // A generous buffer drains the rest of the same flush and reports completion. This must
        // be a single call, not a retry loop: calling `Flush` again after it already completed
        // would ask flate2 to insert another sync marker, so the test only issues exactly the
        // calls this one flush needs.
        let mut generous = [MaybeUninit::uninit(); 256];
        let (step, consumed, _) = codec.step(&[], &mut generous, Operation::Flush).expect("flush succeeds");
        assert_eq!(consumed, 0, "no new input was supplied");
        assert_eq!(step, Step::FlushComplete, "a generous buffer must drain the remainder of the flush");
    }

    #[test]
    fn remaining_output_delegates_to_the_configured_limits() {
        let codec = FlateDecompress::new(
            Wrapper::Zlib,
            FormatLimits::new(None, Some(100)),
            false,
            TrailingData::Reject,
            Pool::disabled().clone(),
        );

        assert_eq!(Codec::remaining_output(&codec, 40), Some(60));
        assert_eq!(Codec::remaining_output(&codec, 100), Some(0));
    }
}
