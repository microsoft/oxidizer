// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Runs the shared benchmark engine's lifecycle/routing tests through rallocator.
//! Miri checks engine ownership with System; allocator internals have their own
//! targeted medium tests, avoiding instrumentation of every harness allocation.

#[cfg(not(miri))]
rallocator::rallocator!();

#[path = "../benches/rallocator_threaded_workloads/fanout.rs"]
mod fanout;
