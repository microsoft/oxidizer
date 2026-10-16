// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Compile-time coverage for the approved unstable Arty host surface.

use std::task::Waker;

use arty_io_host::{DriverHost, IoRuntime};
use static_assertions::{assert_impl_all, assert_not_impl_any};
use thread_aware::{ThreadAware, ThreadBuilder};

assert_impl_all!(IoRuntime: Clone, std::fmt::Debug, Eq, Send, Sync, ThreadAware);
assert_not_impl_any!(DriverHost: Send, Sync);

#[test]
fn approved_public_constructors_are_available() {
    let thread = ThreadBuilder::default().build(std::thread::current().id());
    let mut runtime = IoRuntime::new(observed::Sink::noop());
    runtime.relocate(None, &thread);
    let host = runtime.driver_host(Waker::noop().clone());
    drop(host.waker());
}

#[test]
#[should_panic(expected = "IoRuntime must be relocated onto its worker before creating a driver host")]
fn driver_host_requires_a_relocated_runtime() {
    let _ = IoRuntime::new(observed::Sink::noop()).driver_host(Waker::noop().clone());
}

#[test]
fn driver_host_requires_the_relocation_destination_thread() {
    let thread = ThreadBuilder::default().build(std::thread::current().id());
    let mut runtime = IoRuntime::new(observed::Sink::noop());
    runtime.relocate(None, &thread);

    let panicked = std::thread::spawn(move || {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = runtime.driver_host(Waker::noop().clone());
        }))
        .is_err()
    })
    .join()
    .unwrap();

    assert!(panicked);
}
