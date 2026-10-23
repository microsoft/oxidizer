// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration test for the `Vec::resize` panic-safety guard.
//!
//! The separate test binary isolates its process-global panic hook from
//! concurrently panicking tests.

#![allow(clippy::std_instead_of_core, reason = "test code")]
#![allow(clippy::unwrap_used, reason = "test code")]
#![allow(clippy::clone_on_ref_ptr, reason = "test code")]

use std::panic::{AssertUnwindSafe, catch_unwind, set_hook, take_hook};
use std::sync::Arc as StdArc;
use std::sync::atomic::{AtomicUsize, Ordering};

use multitude::Arena;

#[test]
fn resize_guard_drop_uses_subtraction() {
    struct Ctor(StdArc<AtomicUsize>, usize);
    impl Clone for Ctor {
        fn clone(&self) -> Self {
            let prev = self.0.fetch_add(1, Ordering::SeqCst);
            assert!(prev + 1 < self.1, "planned clone panic at index {prev}");
            Self(self.0.clone(), self.1)
        }
    }

    let counter = StdArc::new(AtomicUsize::new(0));
    // Silence the panic logger for the duration of the unwind.
    let prev = take_hook();
    set_hook(Box::new(|_| {}));
    let result = catch_unwind(AssertUnwindSafe(|| {
        let arena = Arena::new();
        let mut v: multitude::vec::Vec<'_, Ctor> = arena.alloc_vec();
        // Panic on the second clone while resizing an empty vector.
        let template = Ctor(counter.clone(), 2);
        v.resize(3, template);
    }));
    set_hook(prev);
    assert!(result.is_err(), "resize must panic via the planted clone panic");
    let payload = result.unwrap_err();
    let s = payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| payload.downcast_ref::<&'static str>().map(std::string::ToString::to_string))
        .unwrap_or_default();
    assert!(s.contains("planned clone panic"), "unexpected panic payload: {s:?}");
}
