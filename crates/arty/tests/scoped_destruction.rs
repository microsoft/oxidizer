// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Scoped completion waits for the borrowed future to be destroyed.

#![cfg(all(feature = "rt", not(miri)))]

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll};
use std::thread::{self, ThreadId};

use arty::runtime::hook::Panic;
use arty::runtime::{Runtime, WorkersPolicy};

#[derive(Debug, Default, PartialEq, Eq)]
struct DropRecord {
    text: String,
    thread: Option<ThreadId>,
}

#[derive(Debug, PartialEq, Eq)]
struct SendOnlyResult;

struct BorrowingFuture<'a> {
    record: &'a mut DropRecord,
    _local: Rc<()>,
}

impl Future for BorrowingFuture<'_> {
    type Output = SendOnlyResult;

    fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Self::Output> {
        Poll::Ready(SendOnlyResult)
    }
}

impl Drop for BorrowingFuture<'_> {
    fn drop(&mut self) {
        self.record.text.push_str("dropped");
        self.record.thread = Some(thread::current().id());
    }
}

#[test]
fn scoped_return_waits_for_borrowed_non_send_future_destructor() {
    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(1))
        .on(Panic::IGNORE)
        .build()
        .unwrap();
    let worker = runtime.scheduler().block_on(async |_| thread::current().id()).unwrap();
    let mut record = DropRecord::default();
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        runtime.scheduler().block_on(async |_| {
            BorrowingFuture {
                record: &mut record,
                _local: Rc::new(()),
            }
            .await
        })
    }))
    .unwrap();

    assert_eq!(outcome.unwrap(), SendOnlyResult);
    assert_eq!(
        (record, runtime.scheduler().block_on(async |_| 23).unwrap()),
        (
            DropRecord {
                text: "dropped".to_owned(),
                thread: Some(worker),
            },
            23,
        ),
    );
    runtime.stop().unwrap();
}
