// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Compile-time coverage for the approved unstable Arty host surface.

use std::task::Waker;

use arty_io_host::{DriverHost, IoRuntime};
use static_assertions::{assert_impl_all, assert_not_impl_any};
use thread_aware::ThreadBuilder;

assert_impl_all!(IoRuntime: Clone, std::fmt::Debug, Eq, Send, Sync);
assert_not_impl_any!(DriverHost: Send, Sync);

#[test]
fn approved_public_constructors_are_available() {
    let thread = ThreadBuilder::default().build(std::thread::current().id());
    let runtime = IoRuntime::new(observed::Sink::noop());
    let host = runtime.driver_host(thread, Waker::noop().clone()).unwrap();
    drop(host.waker());
}

#[test]
fn driver_host_requires_the_current_worker_thread() {
    let thread = ThreadBuilder::default().build(std::thread::current().id());
    let runtime = IoRuntime::new(observed::Sink::noop());

    let error = std::thread::spawn(move || {
        runtime
            .driver_host(thread, Waker::noop().clone())
            .expect_err("a remote worker coordinate must be rejected")
    })
    .join()
    .unwrap();

    assert_eq!(
        error.to_string(),
        "I/O driver host must be created on the worker identified by its thread coordinate"
    );
}
