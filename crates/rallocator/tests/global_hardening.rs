// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Real container ownership and reallocation through the installed allocator.

#![expect(clippy::unwrap_used, reason = "Container tests fail immediately on invariant violations")]

use std::collections::VecDeque;
use std::hint::black_box;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use allocation_hints::heaps::{Heap, bump};
use allocation_hints::with_hint;

rallocator::rallocator!();

#[repr(C, align(8192))]
struct Aligned {
    id: usize,
    bytes: [u8; 513],
}

impl Aligned {
    fn new(id: usize) -> Self {
        Self {
            id,
            bytes: [u8::try_from(id).unwrap(); 513],
        }
    }

    fn check(&self, id: usize) {
        assert_eq!(self.id, id);
        assert!(self.bytes.iter().all(|byte| usize::from(*byte) == id));
    }
}

#[test]
fn aligned_vectors_grow_shrink_and_escape_bump_heaps() {
    let heap = Heap::bump(bump::Options::new().with_max_alignment(8_192).with_retained_chunks(1));
    let maximum = black_box(if cfg!(miri) { 3 } else { 31 });
    let mut values = with_hint(&heap, || {
        let mut values = Vec::with_capacity(1);
        for id in 0..maximum {
            values.push(Aligned::new(id));
            for (index, value) in values.iter().enumerate() {
                value.check(index);
            }
        }
        values.shrink_to_fit();
        values
    });
    drop(heap);
    values.truncate(maximum / 2);
    values.shrink_to_fit();
    values.reserve_exact(maximum);
    for id in values.len()..maximum {
        values.push(Aligned::new(id));
    }
    let boxed = values.into_boxed_slice();
    assert_eq!(boxed.as_ptr().addr() % 8_192, 0);
    for (index, value) in boxed.iter().enumerate() {
        value.check(index);
    }
    std::thread::spawn(move || {
        for (index, value) in boxed.iter().enumerate() {
            value.check(index);
        }
    })
    .join()
    .unwrap();
}

struct Record {
    id: usize,
    label: String,
    payload: Vec<u8>,
    dropped: Arc<AtomicUsize>,
}

impl Record {
    fn new(id: usize, dropped: &Arc<AtomicUsize>) -> Self {
        let size = [1, 17, 257, 4_097, 16_385][id % 5];
        Self {
            id,
            label: format!("record-{id}"),
            payload: (0..size).map(|offset| u8::try_from((id + offset) % 251).unwrap()).collect(),
            dropped: Arc::clone(dropped),
        }
    }

    fn check(&self) {
        assert_eq!(self.label, format!("record-{}", self.id));
        for (offset, byte) in self.payload.iter().enumerate() {
            assert_eq!(usize::from(*byte), (self.id + offset) % 251);
        }
    }
}

impl Drop for Record {
    fn drop(&mut self) {
        self.dropped.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn nested_containers_outlive_arena_and_producer_thread() {
    let count = black_box(if cfg!(miri) { 4 } else { 64 });
    let dropped = Arc::new(AtomicUsize::new(0));
    let producer_dropped = Arc::clone(&dropped);
    let mut records = std::thread::spawn(move || {
        let heap = Heap::bump(bump::Options::new().with_retained_chunks(1));
        with_hint(&heap, || {
            (0..count)
                .map(|id| Arc::new(Record::new(id, &producer_dropped)))
                .collect::<VecDeque<_>>()
        })
    })
    .join()
    .unwrap();
    let weak: Vec<_> = records.iter().map(Arc::downgrade).collect();
    records.rotate_left(count / 3);
    let mut retained = Vec::with_capacity(count);
    while let Some(record) = records.pop_front() {
        record.check();
        retained.push(Arc::clone(&record));
        std::thread::spawn(move || record.check()).join().unwrap();
    }
    assert_eq!(dropped.load(Ordering::Relaxed), 0);
    std::thread::spawn(move || {
        for record in retained.into_iter().rev() {
            record.check();
        }
    })
    .join()
    .unwrap();
    assert_eq!(dropped.load(Ordering::Relaxed), count);
    assert!(weak.iter().all(|record| record.upgrade().is_none()));
}
