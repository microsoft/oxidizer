// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::Any;
use std::marker::PhantomPinned;
use std::mem::ManuallyDrop;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::pin::Pin;
use std::task::{Context, Poll};

use observed::context::Transfer;
use observed::{Sink, emit};

use crate::runtime::telemetry::events::TaskPanicked;

pub(crate) type Panic = Box<dyn Any + Send + 'static>;

// An ordinary field cannot catch its own destructor panic. Pin<Box<T>> would
// add an allocation to every task. ManuallyDrop keeps storage inline and lets
// us retire it before an in-place, panic-protected drop.
pub(super) struct TaskStorage<T> {
    inner: ManuallyDrop<T>,
    live: bool,
    _pinned: PhantomPinned,
}

impl<T> TaskStorage<T> {
    pub(super) fn new(inner: T) -> Self {
        Self {
            inner: ManuallyDrop::new(inner),
            live: true,
            _pinned: PhantomPinned,
        }
    }

    pub(super) fn is_live(&self) -> bool {
        self.live
    }

    pub(super) fn destroy_pinned(self: Pin<&mut Self>) -> Result<(), Panic> {
        // SAFETY: destruction leaves the retired value in place and never moves it.
        let this = unsafe { self.get_unchecked_mut() };
        Self::destroy(&mut this.inner, &mut this.live)
    }

    fn destroy(inner: &mut ManuallyDrop<T>, live: &mut bool) -> Result<(), Panic> {
        if !std::mem::replace(live, false) {
            return Ok(());
        }
        catch_unwind(AssertUnwindSafe(|| {
            // SAFETY: the live value stays in place and is retired before its
            // destructor runs, so unwinding cannot cause a second destruction.
            unsafe { ManuallyDrop::drop(inner) };
        }))
    }
}

impl<F: Future> Future for TaskStorage<F> {
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // SAFETY: no operation moves inner after pinning; completion and Drop
        // both destroy it in place. PhantomPinned also prevents moving retired
        // ManuallyDrop storage, even when F itself is Unpin.
        let this = unsafe { self.get_unchecked_mut() };
        assert!(this.live, "task future polled after completion");
        // SAFETY: inner occupies the same pinned storage throughout its lifetime.
        let outcome = unsafe { Pin::new_unchecked(&mut *this.inner) }.poll(cx);
        match outcome {
            Poll::Ready(value) => {
                if let Err(panic) = Self::destroy(&mut this.inner, &mut this.live) {
                    resume_unwind(panic);
                }
                Poll::Ready(value)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))] // Wrapper-invariant fallback terminates in a child process.
impl<T> Drop for TaskStorage<T> {
    fn drop(&mut self) {
        if !self.live {
            return;
        }
        if let Err(panic) = Self::destroy(&mut self.inner, &mut self.live) {
            // The execution wrapper owns diagnostics and must retire storage first.
            // Reaching this fallback with a panic is a wrapper invariant violation.
            abort_after_storage_drop_panic(panic);
        }
    }
}

pub(super) struct TaskFactory<F> {
    inner: Option<F>,
    enrichment: Option<Transfer>,
    sink: Option<Sink>,
}

impl<F> TaskFactory<F> {
    pub(super) fn new(inner: F, enrichment: Transfer, sink: Sink) -> Self {
        Self {
            inner: Some(inner),
            enrichment: Some(enrichment),
            sink: Some(sink),
        }
    }

    pub(super) fn into_parts(mut self) -> (F, Transfer, Sink) {
        (
            self.inner.take().expect("a queued factory is consumed exactly once"),
            self.enrichment
                .take()
                .expect("a queued factory retains enrichment until invocation"),
            self.sink.take().expect("a queued factory retains its sink until invocation"),
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
            emit!(
                self.sink.as_ref().expect("a queued factory retains its sink until disposal"),
                TaskPanicked
            );
            discard_panic(panic);
        }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))] // Terminal repeated-panic termination is child-process-only.
pub(crate) fn discard_panic(mut panic: Panic) {
    // Keep finite secondary-payload chains recoverable without allowing an
    // endlessly self-replacing destructor to monopolize a runtime worker.
    const MAX_DISPOSAL_ATTEMPTS: usize = 32;
    for _ in 0..MAX_DISPOSAL_ATTEMPTS {
        match catch_unwind(AssertUnwindSafe(|| drop(panic))) {
            Ok(()) => return,
            Err(secondary) => panic = secondary,
        }
    }
    // Do not leak task-owned resources or unwind through executor teardown.
    abort_after_repeated_payload_panic(panic);
}

#[cfg_attr(coverage_nightly, coverage(off))] // Intentional process termination is covered by child-process contracts.
fn abort_after_storage_drop_panic(_panic: Panic) -> ! {
    std::process::abort();
}

#[cfg_attr(coverage_nightly, coverage(off))] // Intentional process termination is covered by child-process contracts.
fn abort_after_repeated_payload_panic(_panic: Panic) -> ! {
    std::process::abort();
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))] // Test scaffolding is not runtime behavior.
mod tests {
    use std::future::pending;
    use std::pin::pin;

    use super::*;

    #[test]
    fn pinned_storage_destroy_is_idempotent_and_drop_is_quiet_after_retirement() {
        let mut storage = pin!(TaskStorage::new(pending::<()>()));
        assert!(storage.as_ref().is_live());
        storage.as_mut().destroy_pinned().unwrap();
        assert!(!storage.as_ref().is_live());
        storage.as_mut().destroy_pinned().unwrap();
    }

    #[test]
    fn live_storage_drop_runs_the_normal_cleanup_path() {
        struct DropMarker(std::sync::Arc<std::sync::atomic::AtomicBool>);

        impl Drop for DropMarker {
            fn drop(&mut self) {
                self.0.store(true, std::sync::atomic::Ordering::Release);
            }
        }

        let dropped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        drop(TaskStorage::new(DropMarker(std::sync::Arc::clone(&dropped))));
        assert!(dropped.load(std::sync::atomic::Ordering::Acquire));
    }

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

            let sink = Sink::noop();
            drop(TaskFactory::new(PanicOnDrop(&dropped), sink.transfer_context(), sink));
        }));
        result.unwrap();
        assert!(dropped.load(std::sync::atomic::Ordering::Acquire));
    }
}
