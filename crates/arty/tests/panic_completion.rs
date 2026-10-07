// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! A completed task's destructor is still part of task panic containment.

#![cfg(feature = "rt")]
#![cfg(test)]

mod panic_support;

use std::marker::PhantomPinned;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll};

use panic_support::runtime;
use thread_aware::Unaware;

testing_aids::init_tracing!();

struct ReadyDropPanic {
    drops: Arc<AtomicUsize>,
    _pinned: PhantomPinned,
}

impl Future for ReadyDropPanic {
    type Output = u32;

    fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<u32> {
        Poll::Ready(42)
    }
}

impl Drop for ReadyDropPanic {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::SeqCst);
        panic!("completed future destructor panic");
    }
}

#[test]
fn remote_completed_future_drop_is_a_join_error() {
    let runtime = runtime();
    let drops = Arc::new(AtomicUsize::new(0));
    let task = runtime
        .scheduler()
        .spawn_anywhere(Unaware(Arc::clone(&drops)), |_, Unaware(drops)| ReadyDropPanic {
            drops,
            _pinned: PhantomPinned,
        });
    assert!(task.wait().unwrap_err().is_panic());
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    assert_eq!(runtime.scheduler().spawn_anywhere((), |_, ()| async { 7 }).wait().unwrap(), 7);
    runtime.stop().unwrap();
}

#[test]
fn borrowing_completed_future_drop_is_a_root_error() {
    use std::error::Error as _;

    let runtime = runtime();
    let drops = Arc::new(AtomicUsize::new(0));
    let captured = Arc::clone(&drops);
    let error = runtime
        .scheduler()
        .block_on(move |_| ReadyDropPanic {
            drops: captured,
            _pinned: PhantomPinned,
        })
        .unwrap_err();
    assert!(error.source().unwrap().downcast_ref::<arty::task::JoinError>().unwrap().is_panic());
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    assert_eq!(runtime.scheduler().block_on(async |_| 7).unwrap(), 7);
    runtime.stop().unwrap();
}
