// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public harness wrappers must execute and complete their supplied work.

use std::cell::Cell;
use std::future::poll_fn;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::Poll;

testing_aids::init_tracing!();

#[test]
fn gamma_round2_public_abandon_wrapper_returns_the_supplied_completion() {
    let runs = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&runs);
    assert_eq!(
        testing_aids::execute_or_abandon(move || {
            observed.fetch_add(1, Ordering::Relaxed);
            53
        }),
        Some(53)
    );
    assert_eq!(runs.load(Ordering::Relaxed), 1);
}

#[test]
fn gamma_round2_public_async_wrapper_constructs_polls_and_finishes_the_original_future() {
    let constructions = Rc::new(Cell::new(0));
    let polls = Rc::new(Cell::new(0));
    let completed = Rc::new(Cell::new(false));
    let observed_constructions = Rc::clone(&constructions);
    let observed_polls = Rc::clone(&polls);
    let observed_completed = Rc::clone(&completed);
    testing_aids::async_test(move || {
        observed_constructions.set(observed_constructions.get() + 1);
        poll_fn(move |cx| {
            let polled = observed_polls.get() + 1;
            observed_polls.set(polled);
            if polled == 1 {
                cx.waker().wake_by_ref();
                Poll::Pending
            } else {
                observed_completed.set(true);
                Poll::Ready(())
            }
        })
    });
    assert_eq!(constructions.get(), 1);
    assert_eq!(polls.get(), 2);
    assert!(completed.get());
}
