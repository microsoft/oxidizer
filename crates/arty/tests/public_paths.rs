// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public runtime paths, auto traits, and construction signatures.

#![cfg(feature = "rt")]

testing_aids::init_tracing!();

use std::error::Error as StdError;
use std::fmt::Debug;
use std::io;
use std::panic::{RefUnwindSafe, UnwindSafe};

use arty::runtime::{BlockingPoolPolicy, Builtins, Error, ProcessorCount, Runtime, RuntimeBuilder, RuntimeOperations};
use arty::task::{JoinError, JoinHandle, LocalJoinHandle, LocalTaskScheduler, TaskScheduler};
use static_assertions::{assert_impl_all, assert_not_impl_any};
use thread_aware::ThreadAware;

assert_impl_all!(Runtime: Send, Sync, Debug);
assert_not_impl_any!(Runtime: ThreadAware);
assert_impl_all!(RuntimeBuilder: Debug);
assert_impl_all!(Builtins: Send, Sync, Clone, Debug, ThreadAware);
assert_impl_all!(TaskScheduler: Send, Sync, Clone, Debug);
assert_not_impl_any!(TaskScheduler: UnwindSafe, RefUnwindSafe);
assert_impl_all!(JoinHandle<()>: Future, Send);
assert_impl_all!(LocalJoinHandle<()>: Future);
assert_impl_all!(JoinError: StdError, Send, Sync, Debug);
assert_not_impl_any!(LocalJoinHandle<()>: Send, Sync);
assert_not_impl_any!(LocalTaskScheduler: Send, Sync);
assert_impl_all!(RuntimeOperations: Send, Sync, Clone, Debug, ThreadAware);
assert_impl_all!(BlockingPoolPolicy: Clone, Debug);
assert_impl_all!(ProcessorCount: Clone, Copy, Debug, Default);
assert_impl_all!(Error: StdError, Send, Sync);
assert_not_impl_any!(Error: From<io::Error>, From<Box<dyn StdError + Send + Sync>>);

const _: fn(usize) -> ProcessorCount = ProcessorCount::exactly;
const _: fn(usize) -> ProcessorCount = ProcessorCount::at_most;

const _: fn(&Runtime) -> TaskScheduler = Runtime::task_scheduler;
const _: fn(JoinHandle<u32>) -> Result<u32, JoinError> = JoinHandle::wait;

fn assert_join_output<F: Future<Output = Result<u32, JoinError>>>() {}
const _: fn() = assert_join_output::<JoinHandle<u32>>;
const _: fn() = assert_join_output::<LocalJoinHandle<u32>>;
