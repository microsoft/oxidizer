// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::cell::Cell;
use std::sync::{Mutex, MutexGuard};

use gungraun::__internal::InternalBenchRunMode;

static PERF_CONTROL_LOCK: Mutex<()> = Mutex::new(());

thread_local! {
    static ACTIVE: Cell<bool> = const { Cell::new(false) };
}

#[derive(Debug)]
struct SectionLock<'a> {
    _guard: MutexGuard<'a, ()>,
}

impl<'a> SectionLock<'a> {
    fn acquire(lock: &'a Mutex<()>) -> Self {
        assert!(!ACTIVE.get(), "metabench: nested Gungraun perf sections are not supported");
        let guard = lock
            .lock()
            .expect("a previous Gungraun perf section panicked while holding the control lock");
        ACTIVE.set(true);
        Self { _guard: guard }
    }
}

impl Drop for SectionLock<'_> {
    fn drop(&mut self) {
        ACTIVE.set(false);
    }
}

/// Serializes native perf callbacks before they access Gungraun's shared controls.
pub fn run(mode: InternalBenchRunMode, body: impl FnOnce()) {
    let _section = (!matches!(mode, InternalBenchRunMode::Default)).then(|| SectionLock::acquire(&PERF_CONTROL_LOCK));
    body();
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::Barrier;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;

    use super::*;

    #[test]
    fn sections_serialize_access_from_multiple_threads() {
        let lock = Mutex::new(());
        let start = Barrier::new(2);
        let active = AtomicUsize::new(0);
        let completed = AtomicUsize::new(0);

        thread::scope(|scope| {
            for _ in 0..2 {
                scope.spawn(|| {
                    start.wait();
                    for _ in 0..16 {
                        let _section = SectionLock::acquire(&lock);
                        assert_eq!(active.fetch_add(1, Ordering::SeqCst), 0);
                        thread::yield_now();
                        assert_eq!(active.fetch_sub(1, Ordering::SeqCst), 1);
                        completed.fetch_add(1, Ordering::SeqCst);
                    }
                });
            }
        });

        assert_eq!(completed.load(Ordering::SeqCst), 32);
    }

    #[test]
    fn nested_section_panics_without_deadlocking_or_releasing_the_outer_lock() {
        let lock = Mutex::new(());
        let outer = SectionLock::acquire(&lock);
        let error = catch_unwind(|| SectionLock::acquire(&lock)).unwrap_err();
        assert_eq!(
            error
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| error.downcast_ref::<&str>().copied()),
            Some("metabench: nested Gungraun perf sections are not supported")
        );
        lock.try_lock().unwrap_err();
        drop(outer);
        drop(SectionLock::acquire(&lock));
    }

    #[test]
    fn unwinding_releases_the_thread_state_but_poisons_the_control_lock() {
        let lock = Mutex::new(());
        let failure = catch_unwind(AssertUnwindSafe(|| {
            let _section = SectionLock::acquire(&lock);
            panic!("workload failed");
        }));
        assert!(failure.is_err());
        assert!(!ACTIVE.get());
        assert!(lock.is_poisoned());
        catch_unwind(|| SectionLock::acquire(&lock)).unwrap_err();
        drop(SectionLock::acquire(&Mutex::new(())));
    }
}
