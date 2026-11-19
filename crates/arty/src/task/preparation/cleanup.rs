// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::panic::{AssertUnwindSafe, catch_unwind};

use arty_executor::discard_task_panic_payload;

pub(crate) fn discard_panicking_drop<T>(value: T) {
    if let Err(panic) = catch_unwind(AssertUnwindSafe(|| drop(value))) {
        discard_task_panic_payload(panic);
    }
}

#[cfg(test)]
#[test]
fn gamma_survivor_private_preparation_cleanup_disposes_panicking_capture_and_payload_once() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Payload(Arc<AtomicUsize>);
    impl Drop for Payload {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::Relaxed);
            panic!("preparation cleanup panic payload");
        }
    }
    struct Capture(Arc<AtomicUsize>);
    impl Drop for Capture {
        fn drop(&mut self) {
            std::panic::panic_any(Payload(Arc::clone(&self.0)));
        }
    }
    let drops = Arc::new(AtomicUsize::new(0));
    std::panic::catch_unwind(|| discard_panicking_drop(Capture(Arc::clone(&drops)))).unwrap();
    assert_eq!(drops.load(Ordering::Relaxed), 1);
}
