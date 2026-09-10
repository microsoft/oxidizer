// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Routes ordinary containers through a reusable bump heap for one scope.

use allocation_hints::heaps::{Heap, bump};
use allocation_hints::with_hint;

rallocator::rallocator!();

fn main() {
    let heap = Heap::bump(bump::Options::new());
    let sum = with_hint(&heap, || {
        let values = (0..1_000_u64).collect::<Vec<_>>();
        values.iter().sum::<u64>()
    });

    println!("sum: {sum}");
}
