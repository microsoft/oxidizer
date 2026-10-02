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

type Panic = Box<dyn Any + Send + 'static>;

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

impl<T> Drop for TaskStorage<T> {
    fn drop(&mut self) {
        if !self.live {
            return;
        }
        if let Err(panic) = Self::destroy(&mut self.inner, &mut self.live) {
            // The execution wrapper owns diagnostics and must retire storage first.
            // Reaching this fallback with a panic is a wrapper invariant violation.
            drop(panic);
            std::process::abort();
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

pub(super) fn discard_panic(mut panic: Panic) {
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
    std::process::abort();
}
