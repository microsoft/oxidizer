// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};

use arty_executor::discard_task_panic_payload;
use performables::sync::mutex::Mutex;

pub(crate) type ShutdownCallback = Box<dyn FnOnce() + Send + 'static>;

/// Stores callbacks until shutdown completion invokes and clears them.
#[derive(derive_more::Debug)]
pub(crate) struct OnShutdown {
    #[debug(ignore)]
    callbacks: Mutex<Option<VecDeque<ShutdownCallback>>>,
}

impl Default for OnShutdown {
    fn default() -> Self {
        Self {
            callbacks: Mutex::new(Some(VecDeque::new())),
        }
    }
}

impl OnShutdown {
    pub(crate) fn register<F>(&self, callback: F) -> Option<ShutdownCallback>
    where
        F: FnOnce() + Send + 'static,
    {
        let callback = Box::new(callback) as ShutdownCallback;
        let mut callbacks = self.callbacks.lock();
        let Some(callbacks) = callbacks.as_mut() else {
            return Some(callback);
        };
        callbacks.push_back(callback);
        None
    }

    pub(crate) fn invoke_and_clear(&self) {
        let mut first_panic = None;
        loop {
            let callback = {
                let mut callbacks = self.callbacks.lock();
                let Some(callback) = callbacks.as_mut().and_then(VecDeque::pop_front) else {
                    *callbacks = None;
                    break;
                };
                callback
            };
            if let Err(panic) = catch_unwind(AssertUnwindSafe(callback)) {
                if first_panic.is_none() {
                    first_panic = Some(panic);
                } else {
                    discard_task_panic_payload(panic);
                }
            }
        }

        if let Some(panic) = first_panic {
            resume_unwind(panic);
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::OnShutdown;

    #[test]
    fn run_invokes_and_clears_callbacks() {
        let callbacks = OnShutdown::default();
        let calls = Arc::new(AtomicUsize::new(0));
        for _ in 0..2 {
            let calls = Arc::clone(&calls);
            assert!(
                callbacks
                    .register(move || {
                        calls.fetch_add(1, Ordering::SeqCst);
                    })
                    .is_none()
            );
        }

        callbacks.invoke_and_clear();
        callbacks.invoke_and_clear();

        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn callback_panic_does_not_skip_later_callbacks() {
        let callbacks = OnShutdown::default();
        let calls = Arc::new(AtomicUsize::new(0));
        assert!(callbacks.register(|| panic!("first callback")).is_none());
        assert!(callbacks.register(|| panic!("second callback")).is_none());
        let observed_calls = Arc::clone(&calls);
        assert!(
            callbacks
                .register(move || {
                    observed_calls.fetch_add(1, Ordering::SeqCst);
                })
                .is_none()
        );

        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| callbacks.invoke_and_clear())).unwrap_err();

        assert_eq!(panic.downcast_ref::<&str>(), Some(&"first callback"));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn late_registration_returns_the_callback() {
        let callbacks = OnShutdown::default();
        callbacks.invoke_and_clear();
        let called = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&called);

        let callback = callbacks
            .register(move || {
                observed.fetch_add(1, Ordering::SeqCst);
            })
            .expect("registration after invocation must return the callback");

        callback();
        assert_eq!(called.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn callbacks_registered_during_invocation_are_drained() {
        let callbacks = Arc::new(OnShutdown::default());
        let called = Arc::new(AtomicUsize::new(0));
        let nested_callbacks = Arc::clone(&callbacks);
        let nested_called = Arc::clone(&called);
        assert!(
            callbacks
                .register(move || {
                    let nested_called = Arc::clone(&nested_called);
                    assert!(
                        nested_callbacks
                            .register(move || {
                                nested_called.fetch_add(1, Ordering::SeqCst);
                            })
                            .is_none()
                    );
                })
                .is_none()
        );

        callbacks.invoke_and_clear();

        assert_eq!(called.load(Ordering::SeqCst), 1);
        assert!(callbacks.register(|| {}).is_some());
    }
}
