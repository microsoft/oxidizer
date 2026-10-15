// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Compile-time coverage for the approved unstable Arty host surface.

use std::task::Waker;

use arty_io_host::{DriverHost, IoRuntime};
use static_assertions::{assert_impl_all, assert_not_impl_any};
use thread_aware::ThreadBuilder;

assert_impl_all!(IoRuntime: Clone, std::fmt::Debug, Default, Eq, Send, Sync);
assert_not_impl_any!(DriverHost: Send, Sync);

#[test]
fn approved_public_constructors_are_available() {
    let thread = ThreadBuilder::default().build(std::thread::current().id());
    let host = IoRuntime::new().driver_host(thread, Waker::noop().clone(), observed::Sink::noop());
    drop(host.waker());
}
