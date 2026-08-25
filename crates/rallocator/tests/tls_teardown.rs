// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration test for allocator use during thread-local teardown.

use std::hint::black_box;
use std::sync::atomic::{AtomicBool, Ordering};

rallocator::rallocator!();

static DESTRUCTOR_COMPLETED: AtomicBool = AtomicBool::new(false);

struct AllocatingTlsDestructor;

impl Drop for AllocatingTlsDestructor {
    fn drop(&mut self) {
        let mut value = Box::new([0_u8; 64]);
        value[0] = 42;
        black_box(value);
        DESTRUCTOR_COMPLETED.store(true, Ordering::Release);
    }
}

thread_local! {
    static ALLOCATING_TLS_DESTRUCTOR: AllocatingTlsDestructor =
        const { AllocatingTlsDestructor };
}

#[test]
fn allocator_remains_usable_by_later_tls_destructors() {
    rallocator::initialize();
    std::thread::spawn(|| {
        ALLOCATING_TLS_DESTRUCTOR.with(|_| {});

        let value = Box::new([7_u8; 64]);
        assert_eq!(value[0], 7);
    })
    .join()
    .unwrap();

    assert!(DESTRUCTOR_COMPLETED.load(Ordering::Acquire));
}
