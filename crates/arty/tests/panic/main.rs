// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Root-task panics propagate through the runtime test entry point.

#![cfg(feature = "rt")]
#![cfg(test)]

#[path = "../panic_support/mod.rs"]
mod runtime_support;
#[path = "../support/mod.rs"]
mod support;

mod abandoned;
mod abandoned_panics;
mod abandoned_payload_drop;
mod blocking_shutdown;
mod borrowed_wakers;
mod cancellation;
mod completion;
mod factory_disposal;
mod join_notifications;
mod owner_drop;
mod payload_chain;
mod policy;
mod rejected_borrowing;
mod relocation;
mod scoped;
mod self_wake;
mod shutdown_timer;
mod stop_race;
mod waker_lifetimes;

#[cfg(feature = "macros")]
use arty::task::Context;
#[cfg(feature = "macros")]
use arty::test;

// The macro boundary preserves root-task panic payloads, unlike ordinary task joins.
#[cfg(feature = "macros")]
#[test]
#[should_panic(expected = "this is a panic and we expect it to be visible in the test output")]
async fn main(cx: Context) {
    assert_eq!(cx.scheduler().spawn(async |_| 42).await.unwrap(), 42);
    panic!("this is a panic and we expect it to be visible in the test output");
}
