// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration test for the `Vec::resize` panic-safety guard.
//!
//! This test lives in its own integration-test binary deliberately. It
//! installs a process-global panic hook (`std::panic::set_hook`) to silence
//! the default panic logger while it drives an intentional unwind through
//! `catch_unwind`. The panic hook is process-wide state, so running this test
//! alongside the ~450 other tests in the shared `arena.rs` binary (which run
//! in parallel by default) would let the no-op hook suppress panic output from
//! concurrently panicking tests, and the `take_hook`/`set_hook` save-restore
//! could race with a hook installed by another test.
//!
//! Isolating the test in a single-test binary confines the global panic-hook
//! mutation, mirroring the precedent in `crates/cachet/tests/eviction.rs` and
//! `crates/cachet/tests/no_subscriber.rs`.

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
        // Start from EMPTY vec so old_len == 0 ⇒ mutated `/ 0` div-by-zero.
        // Resize to 3: clones template twice, then moves template into last slot.
        // We make the SECOND clone panic.
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
    // Original: panic payload contains "planned clone panic".
    // Mutated (`/`): the Guard drop triggers div-by-zero, aborting the
    // process before catch_unwind sees a payload — process aborts.
    // If we reach this assertion, the test ran without abort; the
    // payload string must be the *planted* one. The mutated version
    // would either abort or surface a divide-by-zero panic.
    assert!(
        s.contains("planned clone panic"),
        "unexpected panic payload: {s:?} (mutated `/ 0` in Guard::drop would surface as divide-by-zero)"
    );
}
