// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Demonstrates how to manually extract the "how many slices in a `BytesView`" metric from an app.
//!
//! This is for internal use only, to help fine-tune the internal memory layout of `BytesView`.
//!
//! Reporting via metrics pipeline (e.g. OpenTelemetry) is also possible but out of scope here.

use bytesbuf::BytesView;
use bytesbuf::mem::GlobalPool;
use nm::Report;

fn main() {
    // In a real-world app, the memory provider would be supplied by the application framework.
    let memory = GlobalPool::new();

    // First a simple view consisting of a single slice.
    let sample1 = BytesView::copied_from_slice(b"Hello, world!", &memory);

    // Which repeated 4 times gives us a sequence made up of 4 slices.
    let sample4 = BytesView::from_views([sample1.clone(), sample1.clone(), sample1.clone(), sample1]);

    // Which repeated 4 times gives us a sequence made up of 16 slices.
    let _sample16 = BytesView::from_views([sample4.clone(), sample4.clone(), sample4.clone(), sample4]);

    // Dump metrics to stdout.
    println!("{}", Report::collect());
}
