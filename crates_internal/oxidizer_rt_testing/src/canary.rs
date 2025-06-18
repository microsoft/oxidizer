// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::pin::Pin;
use std::sync::{Arc, Weak};
use std::task;

/// This is a future that never completes but one whose existence we can observe in tests, to detect
/// whether the future has been dropped or not.
#[derive(Debug)]
pub struct CanaryFuture {
    #[expect(dead_code, reason = "Exists only to keep a reference count")]
    link: Arc<()>,

    started: Option<oneshot::Sender<()>>,
}

impl Future for CanaryFuture {
    type Output = ();

    #[cfg_attr(test, mutants::skip)] // This crate is only used during testing, no point testing our test code.
    fn poll(mut self: Pin<&mut Self>, _cx: &mut task::Context<'_>) -> task::Poll<Self::Output> {
        if let Some(tx) = self.as_mut().started.take() {
            _ = tx.send(());
        }

        task::Poll::Pending
    }
}

impl CanaryFuture {
    /// Creates a new `CanaryFuture`, returning both a start event to be used for observing when
    /// the canary has started, and an observer (weak pointer) to detect when it has been dropped.
    #[cfg_attr(test, mutants::skip)] // This crate is only used during testing, no point testing our test code.
    #[must_use]
    pub fn new_with_start_notification_and_observer() -> (Self, oneshot::Receiver<()>, Weak<()>) {
        let link = Arc::new(());
        let weak = Arc::downgrade(&link);

        let (started_tx, started_rx) = oneshot::channel();

        (
            Self {
                link,
                started: Some(started_tx),
            },
            started_rx,
            weak,
        )
    }
}