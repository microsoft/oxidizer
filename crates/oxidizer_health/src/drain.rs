// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::atomic::{AtomicUsize, Ordering};

/// Runs queued work on exactly one caller at a time without blocking the others.
///
/// A caller that enqueues work and finds a drain already running returns immediately; the running
/// drainer observes the new request and processes the queue again before it stops. This lets
/// publishers and drop handlers hand updates to the owning actor without waiting for it, while
/// the actor state is only ever touched by the single active drainer.
#[derive(Debug, Default)]
pub(crate) struct Drain {
    requests: AtomicUsize,
}

impl Drain {
    /// Requests a drain and runs `process` until no requests remain, unless another caller is
    /// already draining.
    ///
    /// Callers must enqueue their work before calling this method.
    pub(crate) fn run(&self, mut process: impl FnMut()) {
        // AcqRel: Release publishes the caller's queued work to whichever drainer observes this
        // request; Acquire lets a new drainer observe state left by the previous drainer.
        if self.requests.fetch_add(1, Ordering::AcqRel) != 0 {
            return;
        }

        let mut claimed = 1;
        loop {
            process();
            // AcqRel: Acquire observes work queued by callers whose requests arrived during
            // `process`; Release hands this drainer's state to the next one.
            let previous = self.requests.fetch_sub(claimed, Ordering::AcqRel);
            if previous == claimed {
                return;
            }
            claimed = previous - claimed;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    use std::thread;

    use super::*;

    #[test]
    fn nested_request_is_processed_by_running_drainer() {
        let drain = Drain::default();
        let mut runs = 0;
        drain.run(|| {
            runs += 1;
            if runs == 1 {
                drain.run(|| unreachable!("a second drainer must not start"));
            }
        });
        assert_eq!(runs, 2);
    }

    #[test]
    fn concurrent_requests_never_overlap() {
        let drain = Arc::new(Drain::default());
        let active = Arc::new(AtomicBool::new(false));
        let threads: Vec<_> = (0..4)
            .map(|_| {
                let drain = Arc::clone(&drain);
                let active = Arc::clone(&active);
                thread::spawn(move || {
                    for _ in 0..1_000 {
                        drain.run(|| {
                            assert!(!active.swap(true, Ordering::SeqCst));
                            active.store(false, Ordering::SeqCst);
                        });
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
    }
}
