// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Round trips through runtime-selected formats.

use compressors::Resources;
use compressors::format::Format;

#[cfg(all(feature = "deflate", not(feature = "zlib")))]
#[test]
fn afternoon_raw_deflate_pool_restores_raw_headers_without_the_zlib_feature() {
    let resources = Resources::new(bytesbuf::mem::GlobalPool::new()).with_pool_capacity(1);
    let plain = b"raw deflate must retain its framing when a pooled decompressor is reset";
    let compressed = compressors::format::compress(Format::Deflate, plain.as_slice(), &resources).unwrap();
    for _ in 0..3 {
        let decoded = compressors::format::decompress(Format::Deflate, compressed.clone(), &resources).unwrap();
        assert_eq!(decoded.to_vec(), plain);
    }
}

#[test]
fn every_enabled_runtime_format_round_trips() {
    let payload = b"runtime selected format ".repeat(200);

    for &format in Format::ALL {
        let compressed = compressors::format::compress(format, payload.as_slice(), Resources::global()).unwrap();
        let plain = compressors::format::decompress(format, compressed, Resources::global()).unwrap();

        assert_eq!(plain.to_vec(), payload, "{format:?} failed to round trip");
    }
}
