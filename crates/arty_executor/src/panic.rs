// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::Any;
use std::panic::{AssertUnwindSafe, catch_unwind};

/// A panic captured while executing or disposing an executor task.
pub type TaskPanicPayload = Box<dyn Any + Send + 'static>;

/// Disposes a captured task panic without allowing its destructor to start another unwind.
///
/// Cancellation and result delivery sometimes need to discard a panic payload. Payload destructors
/// are user code and may themselves panic, so disposal catches replacement payloads and aborts only
/// if they keep panicking beyond a bounded number of attempts.
#[cfg_attr(coverage_nightly, coverage(off))] // Terminal repeated-panic termination is child-process-only.
pub fn discard_task_panic_payload(mut panic: TaskPanicPayload) {
    const MAX_DISPOSAL_ATTEMPTS: usize = 32;
    for _ in 0..MAX_DISPOSAL_ATTEMPTS {
        match catch_unwind(AssertUnwindSafe(|| drop(panic))) {
            Ok(()) => return,
            Err(secondary) => panic = secondary,
        }
    }
    abort_after_repeated_payload_panic(panic);
}

#[cfg_attr(coverage_nightly, coverage(off))] // Intentional process termination is covered by child-process contracts.
fn abort_after_repeated_payload_panic(_panic: TaskPanicPayload) -> ! {
    std::process::abort();
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    struct PanicOnce(Arc<AtomicUsize>);

    impl Drop for PanicOnce {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::Relaxed);
            panic!("panic payload destructor");
        }
    }

    #[test]
    fn captured_panic_with_panicking_destructor_is_discarded() {
        let drops = Arc::new(AtomicUsize::new(0));
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe({
            let drops = Arc::clone(&drops);
            move || discard_task_panic_payload(Box::new(PanicOnce(drops)))
        }));

        result.unwrap();
        assert_eq!(drops.load(Ordering::Relaxed), 1);
    }
}
