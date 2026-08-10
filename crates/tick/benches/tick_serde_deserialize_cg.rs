// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Callgrind benchmarks for serde timestamp deserialization.
//!
//! Paired with `tick_serde_deserialize.rs`, which covers the same operations
//! under wall-clock and allocation measurement.

#![allow(missing_docs, reason = "no need for API documentation on benchmark code")]
#![cfg_attr(
    target_os = "linux",
    expect(
        clippy::exit,
        clippy::missing_docs_in_private_items,
        unused_qualifications,
        reason = "Triggered by Gungraun macro expansion. Upstream tracking issues are pending."
    )
)]

#[cfg(not(target_os = "linux"))]
fn main() {
    // Gungraun requires Valgrind, which is Linux-only.
}

#[cfg(target_os = "linux")]
mod linux {
    use std::hint::black_box;

    use gungraun::{library_benchmark, library_benchmark_group};
    use tick::fmt::{EcmaScript, Iso8601, Rfc2822, UnixSeconds};

    const ISO_8601: &str = r#""2024-08-06T21:30:00Z""#;
    const RFC_2822: &str = r#""Tue, 06 Aug 2024 21:30:00 GMT""#;
    const UNIX_SECONDS: &str = "1722979800";
    const ECMASCRIPT: &str = r#""2024-08-06T21:30:00.123Z""#;

    fn warm_rfc_2822() {
        _ = serde_json::from_str::<Rfc2822>(RFC_2822).unwrap();
    }

    #[library_benchmark]
    fn formats_iso_8601() -> Iso8601 {
        serde_json::from_str(black_box(ISO_8601)).unwrap()
    }

    #[library_benchmark]
    #[bench::run(warm_rfc_2822())]
    fn formats_rfc_2822(_: ()) -> Rfc2822 {
        serde_json::from_str(black_box(RFC_2822)).unwrap()
    }

    #[library_benchmark]
    fn formats_unix_seconds() -> UnixSeconds {
        serde_json::from_str(black_box(UNIX_SECONDS)).unwrap()
    }

    #[library_benchmark]
    fn formats_ecmascript() -> EcmaScript {
        serde_json::from_str(black_box(ECMASCRIPT)).unwrap()
    }

    library_benchmark_group!(
        name = formats;
        benchmarks = formats_iso_8601, formats_rfc_2822, formats_unix_seconds, formats_ecmascript
    );
}

#[cfg(target_os = "linux")]
use gungraun::{Callgrind, LibraryBenchmarkConfig};
#[cfg(target_os = "linux")]
pub use linux::formats;

#[cfg(target_os = "linux")]
gungraun::main!(
    config = LibraryBenchmarkConfig::default()
        .tool(Callgrind::with_args(["--branch-sim=yes"]));
    library_benchmark_groups = formats
);
