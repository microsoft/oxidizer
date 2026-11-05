// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! In-memory compression contracts and error boundaries.

#![allow(clippy::unwrap_used, reason = "test code")]

use std::error::Error as _;
use std::num::{NonZeroU32, NonZeroU64, NonZeroUsize};

use bytesbuf::BytesView;
use compressors::format::Format;
use compressors::{CompressorBuilder, DecompressorBuilder, DecompressorLimits, Error, Resources, TrailingData};
use recoverable::{Recovery as _, RecoveryInfo};

#[cfg(feature = "deflate")]
#[test]
fn gamma_round2_raw_decoder_reuse_does_not_add_a_zlib_header_requirement() {
    let resources = Resources::default().with_pool_capacity(1);
    for payload in [b"first".as_slice(), b"SECOND".as_slice(), b"third".as_slice()] {
        let encoded = compressors::format::compress(Format::Deflate, payload, &resources).unwrap();
        let decoded = compressors::format::decompress(Format::Deflate, encoded, &resources).unwrap();
        assert_eq!(decoded.to_vec(), payload);
    }
}

#[cfg(feature = "gzip")]
#[test]
fn gamma_round2_gzip_retains_long_distance_window_history() {
    let mut state = 0x1234_5678_u32;
    let block: Vec<u8> = (0..20_000)
        .map(|_| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            state.to_be_bytes()[0]
        })
        .collect();
    let payload = [block.as_slice(), block.as_slice()].concat();
    let resources = Resources::default();
    let compressed = compressors::format::compress(Format::Gzip, payload.as_slice(), &resources).unwrap();
    assert!(
        compressed.len() < 30_000,
        "the second block must reuse history more than 16 KiB behind it"
    );
    let decoder = DecompressorBuilder::new()
        .output_chunk_size(NonZeroUsize::new(1_024).unwrap())
        .build_format(Format::Gzip, &resources)
        .unwrap();
    let plain = compressors::decompress(compressed, decoder).unwrap();
    assert_eq!(plain.to_vec(), payload);
}

#[test]
fn gamma_round2_relocation_preserves_the_optional_source_coordinate() {
    use std::sync::{Arc, Mutex};

    use bytesbuf::mem::{CallbackMemory, GlobalPool};
    use thread_aware::{Relocator, Thread, ThreadAware};

    #[derive(Clone, Debug)]
    struct RecordsSource {
        memory: GlobalPool,
        sources: Arc<Mutex<Vec<bool>>>,
    }

    impl ThreadAware for RecordsSource {
        fn relocate(&mut self, source: Option<&Thread>, _destination: &Thread) {
            self.sources.lock().unwrap().push(source.is_some());
        }
    }

    fn reserve(data: &RecordsSource, size: usize) -> bytesbuf::BytesBuf {
        data.memory.reserve(size)
    }

    let sources = Arc::new(Mutex::new(Vec::new()));
    let memory = CallbackMemory::new(
        RecordsSource {
            memory: GlobalPool::new(),
            sources: Arc::clone(&sources),
        },
        reserve,
    );
    let mut resources = Resources::new(memory);
    let _ = Relocator::between_threads().source(true).relocate(&mut resources);
    let _ = Relocator::between_threads().source(false).relocate(&mut resources);
    assert_eq!(*sources.lock().unwrap(), [true, false]);
}

#[test]
fn gamma_survivor_chunked_roundtrip_truncation_and_trailing_data() {
    let resources = Resources::default();
    let payload: Vec<u8> = (0_u16..257).map(|i| u8::try_from(i % 251).unwrap()).collect();
    for &format in Format::ALL {
        for size in [1, 7, 64] {
            let chunk = NonZeroUsize::new(size).unwrap();
            let encoder = CompressorBuilder::new()
                .output_chunk_size(chunk)
                .build_format(format, &resources)
                .unwrap();
            let compressed = compressors::compress(BytesView::copied_from_slice(&payload, resources.memory()), encoder).unwrap();
            let decoder = DecompressorBuilder::new()
                .output_chunk_size(chunk)
                .build_format(format, &resources)
                .unwrap();
            assert_eq!(compressors::decompress(compressed.clone(), decoder).unwrap().to_vec(), payload);
            let truncated = compressed.range(..compressed.len() - 1);
            let error = compressors::format::decompress(format, truncated, &resources).unwrap_err();
            assert!(
                error.is_corrupt_data() || error.is_unexpected_end_of_stream(),
                "{format:?}: {error}"
            );
            if error.is_unexpected_end_of_stream() {
                assert_eq!(error.to_string(), "compressed stream ended before the final block");
            }

            let trailing = BytesView::from_views([compressed.clone(), BytesView::copied_from_slice(b"not a frame", resources.memory())]);
            let decoder = DecompressorBuilder::new()
                .multi_stream(false)
                .build_format(format, &resources)
                .unwrap();
            let error = compressors::decompress(trailing.clone(), decoder).unwrap_err();
            assert!(error.is_corrupt_data());
            assert_eq!(error.to_string(), "trailing data followed the compressed stream");
            let decoder = DecompressorBuilder::new()
                .multi_stream(false)
                .trailing_data(TrailingData::Ignore)
                .build_format(format, &resources)
                .unwrap();
            assert_eq!(compressors::decompress(trailing, decoder).unwrap().to_vec(), payload);
        }
    }
}

#[test]
fn gamma_survivor_output_limit_exact_boundary_and_message() {
    let resources = Resources::default();
    for &format in Format::ALL {
        let compressed = compressors::format::compress(format, b"abcdefghijklmnop", &resources).unwrap();
        for maximum in [15, 16, 17] {
            let limits = DecompressorLimits::new().max_output_len(NonZeroU64::new(maximum).unwrap());
            let result = compressors::format::decompress_with_limits(format, compressed.clone(), &resources, limits);
            if maximum < 16 {
                let error = result.unwrap_err();
                assert!(error.is_limit_exceeded());
                assert_eq!(error.to_string(), "decompressed output reached 16 bytes, exceeding the limit of 15");
            } else {
                assert_eq!(result.unwrap().to_vec(), b"abcdefghijklmnop");
            }
        }
    }
}

#[test]
fn gamma_survivor_concatenated_members_reset_and_obey_stream_cap() {
    let resources = Resources::default();
    for &format in Format::ALL {
        let first = compressors::format::compress(format, b"first", &resources).unwrap();
        let second = compressors::format::compress(format, b"SECOND", &resources).unwrap();
        let both = BytesView::from_views([first.clone(), second.clone()]);
        let decoder = DecompressorBuilder::new()
            .multi_stream(true)
            .output_chunk_size(NonZeroUsize::new(1).unwrap())
            .limits(DecompressorLimits::new().max_streams(NonZeroU64::new(2).unwrap()))
            .build_format(format, &resources)
            .unwrap();
        assert_eq!(
            compressors::decompress(both.clone(), decoder).unwrap().to_vec(),
            b"firstSECOND",
            "{format:?}"
        );
        let third = BytesView::from_views([both, first]);
        let decoder = DecompressorBuilder::new()
            .multi_stream(true)
            .limits(DecompressorLimits::new().max_streams(NonZeroU64::new(2).unwrap()))
            .build_format(format, &resources)
            .unwrap();
        let error = compressors::decompress(third, decoder).unwrap_err();
        assert!(error.is_limit_exceeded(), "{format:?}: {error}");
        assert_eq!(error.to_string(), "decoded stream count reached 3, exceeding the limit of 2");
    }
}

#[test]
fn gamma_survivor_explicit_limits_keep_all_overrides() {
    let expected = DecompressorLimits::new()
        .max_ratio(NonZeroU32::new(3).unwrap())
        .max_output_len(NonZeroU64::new(17).unwrap())
        .max_streams(NonZeroU64::new(2).unwrap());
    let fallback = DecompressorLimits::new()
        .max_ratio(NonZeroU32::new(9).unwrap())
        .max_output_len(NonZeroU64::new(99).unwrap())
        .max_streams(NonZeroU64::new(5).unwrap());
    assert_eq!(expected.with_fallbacks(fallback), expected);
    let unbounded = DecompressorLimits::new()
        .unbounded_ratio()
        .unbounded_output_len()
        .unbounded_streams();
    assert_eq!(unbounded.with_fallbacks(fallback), unbounded);
}

#[test]
fn gamma_survivor_resources_uses_the_supplied_memory() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use bytesbuf::mem::{CallbackMemory, GlobalPool};
    static RESERVATIONS: AtomicUsize = AtomicUsize::new(0);
    fn reserve(memory: &GlobalPool, size: usize) -> bytesbuf::BytesBuf {
        RESERVATIONS.fetch_add(1, Ordering::Relaxed);
        memory.reserve(size)
    }
    let resources = Resources::new(CallbackMemory::new(GlobalPool::new(), reserve));
    let before = RESERVATIONS.load(Ordering::Relaxed);
    let buffer = resources.memory().reserve(17);
    assert!(buffer.remaining_capacity() >= 17);
    assert_eq!(RESERVATIONS.load(Ordering::Relaxed), before + 1);
}

#[test]
fn gamma_survivor_nested_source_recovery_and_ownership() {
    let inner = Error::other("inner", std::io::Error::from(std::io::ErrorKind::TimedOut));
    let outer = Error::other("outer", inner);
    assert!(outer.is_source());
    assert_eq!(outer.recovery(), RecoveryInfo::from(std::io::ErrorKind::TimedOut));
    assert_eq!(outer.to_string(), "outer");
    assert_eq!(outer.source().unwrap().to_string(), "inner");
    let source = outer.into_source().unwrap();
    assert!(source.downcast::<Error>().unwrap().into_source().unwrap().is::<std::io::Error>());
}

#[cfg(feature = "brotli")]
#[test]
fn gamma_survivor_brotli_window_conversion_keeps_nondefault_value() {
    for exponent in 10..=24 {
        assert_eq!(u8::from(compressors::brotli::WindowSize::new(exponent).unwrap()), exponent);
    }
    let error = compressors::format::decompress(Format::Brotli, [255; 16].as_slice(), Resources::global()).unwrap_err();
    assert!(error.is_corrupt_data());
    assert_eq!(error.to_string(), "the compressed data is not a valid brotli stream");
}

#[cfg(feature = "zstd")]
#[test]
fn gamma_survivor_zstd_level_low_boundary_and_conversion() {
    use compressors::zstd::CompressionLevel;
    let minimum = CompressionLevel::min().get();
    assert_eq!(CompressionLevel::new(minimum - 1), None);
    for level in [minimum, -1, 1, 5, CompressionLevel::max().get()] {
        assert_eq!(i32::from(CompressionLevel::new(level).unwrap()), level);
    }
}

#[cfg(all(feature = "futures-stream", feature = "gzip"))]
#[test]
fn gamma_survivor_stream_source_failure_is_fused() {
    use futures::{StreamExt as _, stream};
    futures::executor::block_on(async {
        let source = stream::iter([
            Err::<BytesView, _>(std::io::Error::from(std::io::ErrorKind::PermissionDenied)),
            Ok(BytesView::copied_from_slice(b"must not be read", Resources::global().memory())),
        ]);
        let mut stream = compressors::CompressionStream::compress(source, compressors::gzip::Compressor::new(Resources::global()));
        let mut failure = None;
        for _ in 0..4 {
            match stream.next().await.unwrap() {
                Ok(_) => {}
                Err(error) => {
                    failure = Some(error);
                    break;
                }
            }
        }
        let error = failure.unwrap();
        assert!(error.is_source());
        assert_eq!(error.to_string(), "the underlying stream failed");
        assert_eq!(
            error.source().unwrap().downcast_ref::<std::io::Error>().unwrap().kind(),
            std::io::ErrorKind::PermissionDenied
        );
        assert!(stream.next().await.is_none());
        assert!(stream.next().await.is_none());
    });
}
