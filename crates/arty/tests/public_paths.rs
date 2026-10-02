// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public runtime paths, auto traits, and construction signatures.

#![cfg(feature = "rt")]

testing_aids::init_tracing!();

use std::error::Error as StdError;
use std::fmt::Debug;
use std::io;
use std::panic::{RefUnwindSafe, UnwindSafe};

use arty::runtime::{BlockingPoolPolicy, Error, ProcessorCount, Runtime, RuntimeBuilder, RuntimeOperations};
use arty::task::{Builtins, JoinError, JoinHandle, LocalJoinHandle, LocalTaskScheduler, RuntimeScheduler, TaskScheduler};
use static_assertions::{assert_impl_all, assert_not_impl_any};
use thread_aware::ThreadAware;

assert_impl_all!(Runtime: Send, Sync, Debug);
assert_not_impl_any!(Runtime: ThreadAware);
assert_impl_all!(RuntimeBuilder: Debug);
assert_impl_all!(Builtins: Send, Sync, Clone, Debug, ThreadAware,
    AsRef<TaskScheduler>, AsRef<arty::time::Clock>, AsRef<arty::time::SimpleClock>, AsRef<observed::Sink>);
assert_impl_all!(TaskScheduler: Send, Sync, Clone, Debug);
assert_not_impl_any!(TaskScheduler: UnwindSafe, RefUnwindSafe);
assert_impl_all!(JoinHandle<()>: Future, Send);
assert_impl_all!(LocalJoinHandle<()>: Future);
assert_impl_all!(JoinError: StdError, Send, Sync, Debug);
assert_not_impl_any!(LocalJoinHandle<()>: Send, Sync);
assert_not_impl_any!(LocalTaskScheduler: Send, Sync);
assert_impl_all!(RuntimeOperations: Send, Sync, Clone, Debug, From<&'static Builtins>, From<&'static Runtime>);
assert_not_impl_any!(RuntimeOperations: ThreadAware);
assert_impl_all!(RuntimeScheduler: Send, Sync, Debug);
assert_not_impl_any!(RuntimeScheduler: Clone, ThreadAware);
assert_impl_all!(BlockingPoolPolicy: Clone, Debug);
assert_impl_all!(ProcessorCount: Clone, Copy, Debug, Default);
assert_impl_all!(Error: StdError, Send, Sync);
assert_not_impl_any!(Error: From<io::Error>, From<Box<dyn StdError + Send + Sync>>);

const _: fn(usize) -> ProcessorCount = ProcessorCount::exactly;
const _: fn(usize) -> ProcessorCount = ProcessorCount::at_most;

const _: fn(&Runtime) -> &RuntimeScheduler = Runtime::scheduler;
const _: fn(Runtime) -> Result<(), Error> = Runtime::stop;
const _: fn(&RuntimeOperations) = RuntimeOperations::request_stop;
const _: fn(&RuntimeOperations, &arty::core::Thread) -> Result<(), Error> = RuntimeOperations::pin_to;
const _: fn(JoinHandle<u32>) -> Result<u32, JoinError> = JoinHandle::wait;

fn assert_join_output<F: Future<Output = Result<u32, JoinError>>>() {}
const _: fn() = assert_join_output::<JoinHandle<u32>>;
const _: fn() = assert_join_output::<LocalJoinHandle<u32>>;
