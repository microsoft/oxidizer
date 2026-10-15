// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::marker::PhantomPinned;
use std::mem::ManuallyDrop;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::pin::Pin;
use std::task::{Context, Poll};

use arty_executor::{TaskPanicPayload, discard_task_panic_payload};

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

    pub(super) fn destroy_pinned(self: Pin<&mut Self>) -> Result<(), TaskPanicPayload> {
        // SAFETY: destruction leaves the retired value in place and never moves it.
        let this = unsafe { self.get_unchecked_mut() };
        Self::destroy(&mut this.inner, &mut this.live)
    }

    fn destroy(inner: &mut ManuallyDrop<T>, live: &mut bool) -> Result<(), TaskPanicPayload> {
        if !std::mem::replace(live, false) {
            return Ok(());
        }
        catch_unwind(AssertUnwindSafe(|| {
            // SAFETY: the live value stays in place and is retired before its destructor runs.
            unsafe { ManuallyDrop::drop(inner) };
        }))
    }
}

impl<F: Future> Future for TaskStorage<F> {
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // SAFETY: inner occupies the same pinned storage throughout its lifetime.
        let this = unsafe { self.get_unchecked_mut() };
        assert!(this.live, "task future polled after completion");
        // SAFETY: no operation moves inner after pinning.
        let outcome = unsafe { Pin::new_unchecked(&mut *this.inner) }.poll(cx);
        if outcome.is_ready()
            && let Err(panic) = Self::destroy(&mut this.inner, &mut this.live)
        {
            resume_unwind(panic);
        }
        outcome
    }
}

#[cfg_attr(coverage_nightly, coverage(off))] // Wrapper-invariant fallback terminates the process.
impl<T> Drop for TaskStorage<T> {
    fn drop(&mut self) {
        if let Err(panic) = Self::destroy(&mut self.inner, &mut self.live) {
            discard_task_panic_payload(panic);
            std::process::abort();
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::future::pending;
    use std::pin::pin;

    use super::*;

    #[test]
    fn pinned_storage_destruction_is_idempotent() {
        let mut storage = pin!(TaskStorage::new(pending::<()>()));

        storage.as_mut().destroy_pinned().unwrap();
        storage.as_mut().destroy_pinned().unwrap();
    }
}
