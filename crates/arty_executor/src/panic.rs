// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::Any;
use std::panic::{AssertUnwindSafe, catch_unwind};

/// A panic captured while executing or disposing an executor task.
pub type TaskPanicPayload = Box<dyn Any + Send + 'static>;

/// Drops a captured task panic without allowing its destructor to unwind.
///
/// Cancellation and result delivery sometimes discard a panic payload. A payload
/// destructor is user code and can panic. This function catches each replacement
/// payload. It aborts the process after a limited number of consecutive destructor
/// panics. The limit prevents an endless replacement chain during executor cleanup.
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
#[cfg_attr(
    not(test),
    expect(
        clippy::needless_pass_by_value,
        reason = "termination consumes the final panic payload without running its destructor"
    )
)]
fn abort_after_repeated_payload_panic(panic: TaskPanicPayload) -> ! {
    #[cfg(not(test))]
    let _ = &panic;
    #[cfg(test)]
    if ABORT_PROBE.get() {
        std::panic::resume_unwind(Box::new(AbortObserved { _payload: panic }));
    }
    std::process::abort();
}

#[cfg(test)]
thread_local! {
    static ABORT_PROBE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
struct AbortObserved {
    _payload: TaskPanicPayload,
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[test]
    fn nov4_repeated_payload_disposal_requests_abort_after_exactly_32_attempts() {
        struct Repeats {
            attempts: Arc<AtomicUsize>,
            keep_panicking: Arc<std::sync::atomic::AtomicBool>,
        }

        impl Drop for Repeats {
            fn drop(&mut self) {
                if self.keep_panicking.load(Ordering::Relaxed) {
                    self.attempts.fetch_add(1, Ordering::Relaxed);
                    std::panic::panic_any(Self {
                        attempts: Arc::clone(&self.attempts),
                        keep_panicking: Arc::clone(&self.keep_panicking),
                    });
                }
            }
        }

        let attempts = Arc::new(AtomicUsize::new(0));
        let keep_panicking = Arc::new(std::sync::atomic::AtomicBool::new(true));
        ABORT_PROBE.set(true);
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            discard_task_panic_payload(Box::new(Repeats {
                attempts: Arc::clone(&attempts),
                keep_panicking: Arc::clone(&keep_panicking),
            }));
        }));
        ABORT_PROBE.set(false);
        let requested_abort = outcome.as_ref().is_err_and(|panic| panic.is::<AbortObserved>());
        keep_panicking.store(false, Ordering::Relaxed);
        drop(outcome);
        assert!(requested_abort);
        assert_eq!(attempts.load(Ordering::Relaxed), 32);
    }

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
