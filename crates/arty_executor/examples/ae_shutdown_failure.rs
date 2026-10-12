// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Demonstrates how a retained join handle reports executor shutdown.

use std::future::pending;
use std::pin::Pin;
use std::task::{Context, Poll, Waker};

use arty_executor::Executor;

fn main() {
    let executor = Executor::builder().build();
    let mut task = executor.tasks().add(pending::<()>());

    assert!(executor.has_tasks());
    drop(executor);

    let Poll::Ready(result) = Pin::new(&mut task).poll(&mut Context::from_waker(Waker::noop())) else {
        std::process::abort();
    };
    let error = result.expect_err("a cancelled pending task cannot produce a result");
    assert!(error.is_shutdown());
}
