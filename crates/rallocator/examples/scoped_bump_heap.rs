// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(clippy::unwrap_used, reason = "The example exits immediately if heap inspection fails")]

//! Routes ordinary containers through a reusable bump heap for one scope.

use allocation_hints::heap::Heap;
use allocation_hints::heap::bump::Options;
use allocation_hints::with_hint;

rallocator::rallocator!();

fn main() {
    rallocator::initialize();

    let heap = Heap::from_thread_pool(Options::new());
    let sum = with_hint(&heap, || {
        let values = (0..1_000_u64).collect::<Vec<_>>();
        values.iter().sum::<u64>()
    });

    println!("sum: {sum}, heap empty: {}", heap.usage().unwrap().is_empty());
}
