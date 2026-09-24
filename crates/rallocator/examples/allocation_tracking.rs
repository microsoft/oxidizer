// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(
    clippy::cast_possible_truncation,
    clippy::needless_collect,
    clippy::unwrap_used,
    reason = "The example keeps retained allocations visible and uses small bounded worker identifiers"
)]

//! Demonstrates caller tracking, scoped heaps, and snapshot capture.

use std::collections::HashMap;
use std::hint::black_box;
use std::sync::Arc;

use allocation_hints::heaps::{Heap, bump};
use allocation_hints::with_hint;
use seismograph::recorder::Configuration;

rallocator::rallocator!();

fn main() {
    seismograph::recorder(Configuration {
        allocations: seismograph::recorder::RecordingPolicy {
            enabled: true,
            capture_backtraces: true,
            ..Default::default()
        },
        ..Default::default()
    });

    let workers: Vec<_> = (0..4)
        .map(|worker| std::thread::spawn(move || worker_allocations(worker)))
        .collect();
    let retained: Vec<_> = workers
        .into_iter()
        .map(|worker| worker.join().expect("worker must not panic"))
        .collect();

    let heap = Heap::bump(bump::Options::new());
    let scoped = with_hint(&heap, || vec![1, 2, 3]);
    let live = seismograph::snapshot(seismograph::snapshot::SnapshotOptions::default()).unwrap();
    live.write_file("snapshot-live.seismograph").unwrap();

    drop((scoped, retained));
    seismograph::recorder(Configuration::default());

    let after_drop = seismograph::snapshot(seismograph::snapshot::SnapshotOptions::default()).unwrap();
    after_drop.write_file("snapshot-after-drop.seismograph").unwrap();
}

#[inline(never)]
fn worker_allocations(worker: usize) -> Vec<u8> {
    for round in 0..50 {
        let vectors: Vec<_> = (0..24)
            .map(|index| {
                let length = 8 + ((worker * 97 + round * 31 + index * 17) % 512);
                vec![(worker ^ round ^ index) as u64; length]
            })
            .collect();

        let map: HashMap<_, _> = (0..128)
            .map(|index| {
                let key = ((worker as u64) << 48) | ((round as u64) << 32) | index;
                (key, Arc::new([key; 8]))
            })
            .collect();

        black_box((&vectors, &map));
    }

    vec![worker as u8; 16 * 1024]
}
