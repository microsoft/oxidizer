// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

extern crate std;

use core::cell::Cell;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};

std::thread_local! {
    static FENCES: Cell<usize> = const { Cell::new(0) };
    static INTERCEPT_ABORT: Cell<bool> = const { Cell::new(false) };
    static SLOT_LIMIT: Cell<Option<u64>> = const { Cell::new(None) };
}

struct RefcountAbort;

pub(crate) fn record_fence() {
    FENCES.set(FENCES.get() + 1);
}

pub(crate) fn take_fences() -> usize {
    FENCES.replace(0)
}

pub(crate) fn intercept_abort() {
    if INTERCEPT_ABORT.get() {
        std::panic::resume_unwind(std::boxed::Box::new(RefcountAbort));
    }
}

pub(crate) fn abort_was_requested(run: impl FnOnce()) -> bool {
    let previous = INTERCEPT_ABORT.replace(true);
    let outcome = catch_unwind(AssertUnwindSafe(run));
    INTERCEPT_ABORT.set(previous);
    match outcome {
        Ok(()) => false,
        Err(payload) if payload.is::<RefcountAbort>() => true,
        Err(payload) => resume_unwind(payload),
    }
}

pub(crate) fn slot_limit() -> Option<u64> {
    SLOT_LIMIT.get()
}

pub(crate) fn with_slot_limit(limit: u64, run: impl FnOnce()) {
    struct Reset(Option<u64>);
    impl Drop for Reset {
        fn drop(&mut self) {
            SLOT_LIMIT.set(self.0);
        }
    }
    let _reset = Reset(SLOT_LIMIT.replace(Some(limit)));
    run();
}

#[test]
fn nov4_final_owner_paths_dispatch_exactly_their_required_acquire_fence() {
    use crate::{MultiPool, Pool};

    let pool = Pool::<u32>::new();
    take_fences();
    drop(pool);
    assert_eq!(take_fences(), 1);

    let pool = MultiPool::new();
    drop(pool.alloc_box(7_u32));
    take_fences();
    drop(pool);
    assert_eq!(take_fences(), 1);

    let pool = Pool::<u32>::new();
    let value = pool.alloc_box(11);
    drop(pool);
    take_fences();
    drop(value);
    assert_eq!(take_fences(), 1);

    let pool = Pool::<u32>::new();
    let first = pool.alloc_arc(13);
    let second = first.clone();
    take_fences();
    drop(first);
    assert_eq!(take_fences(), 0);
    drop(second);
    assert_eq!(take_fences(), 1);
    drop(pool);
    assert_eq!(take_fences(), 1);
}
