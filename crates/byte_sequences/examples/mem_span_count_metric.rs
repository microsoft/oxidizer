// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Demonstrates how to manually extract the "how many spans in a sequence" metric from an app.
//!
//! This is for internal use only, to help fine-tune the internal memory layout of byte sequences.
//!
//! Reporting via metrics pipeline (e.g. OpenTelemetry) is also possible but out of scope here.

use byte_sequences::{BytesView, GlobalMemoryPool};
use nm::Report;

fn main() {
    // In a real-world app, the memory provider would be supplied by the application framework.
    let memory = GlobalMemoryPool::new();

    // First a simple sequence with a single span.
    let sample1 = BytesView::copy_from_slice(b"Hello, world!", &memory);

    // Which repeated 4 times gives us a sequence made up of 4 spans.
    let sample4 = BytesView::from_sequences([sample1.clone(), sample1.clone(), sample1.clone(), sample1]);

    // Which repeated 4 times gives us a sequence made up of 16 spans.
    let _sample16 = BytesView::from_sequences([sample4.clone(), sample4.clone(), sample4.clone(), sample4]);

    // Dump metrics to stdout.
    println!("{}", Report::collect());
}
