// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::panic::{AssertUnwindSafe, catch_unwind};

use arty_executor::discard_task_panic;
use observed::context::Transfer;

pub(super) struct TaskFactory<F> {
    inner: Option<F>,
    enrichment: Option<Transfer>,
}

impl<F> TaskFactory<F> {
    pub(super) fn new(inner: F, enrichment: Transfer) -> Self {
        Self {
            inner: Some(inner),
            enrichment: Some(enrichment),
        }
    }

    pub(super) fn into_parts(mut self) -> (F, Transfer) {
        (
            self.inner.take().expect("a queued factory is consumed exactly once"),
            self.enrichment
                .take()
                .expect("a queued factory retains enrichment until invocation"),
        )
    }
}

impl<F> Drop for TaskFactory<F> {
    fn drop(&mut self) {
        let Some(factory) = self.inner.take() else {
            return;
        };
        let _guard = self
            .enrichment
            .as_ref()
            .expect("a queued factory retains enrichment until disposal")
            .apply_current_thread();
        if let Err(panic) = catch_unwind(AssertUnwindSafe(|| drop(factory))) {
            discard_task_panic(panic);
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))] // Test scaffolding is not runtime behavior.
mod tests {
    use super::*;

    #[test]
    fn queued_factory_disposes_a_panicking_factory_without_unwinding() {
        let dropped = std::sync::atomic::AtomicBool::new(false);
        let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
            struct PanicOnDrop<'a>(&'a std::sync::atomic::AtomicBool);

            impl Drop for PanicOnDrop<'_> {
                fn drop(&mut self) {
                    self.0.store(true, std::sync::atomic::Ordering::Release);
                    panic!("factory drop");
                }
            }

            let sink = observed::Sink::noop();
            drop(TaskFactory::new(PanicOnDrop(&dropped), sink.transfer_context()));
        }));
        result.unwrap();
        assert!(dropped.load(std::sync::atomic::Ordering::Acquire));
    }
}
