// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public runtime paths, auto traits, and construction signatures.

#![cfg(feature = "rt")]

testing_aids::init_tracing!();

use std::error::Error as StdError;
use std::fmt::Debug;
use std::io;
use std::num::NonZeroUsize;
use std::panic::{RefUnwindSafe, UnwindSafe};

use arty::rt::config::{BuildError, ProcessorCount, RuntimeBuilder, WorkerPoolPolicy};
use arty::rt::{Builtins, Error, JoinHandle, LocalJoinHandle, LocalTaskScheduler, Runtime, RuntimeOperations, TaskScheduler};
use static_assertions::{assert_impl_all, assert_not_impl_any, assert_type_eq_all};
use thread_aware::ThreadAware;

assert_impl_all!(Runtime: Send, Sync, Debug);
assert_not_impl_any!(Runtime: ThreadAware);
assert_impl_all!(RuntimeBuilder: Debug);
assert_impl_all!(Builtins: Send, Sync, Clone, Debug, ThreadAware);
assert_impl_all!(TaskScheduler: Send, Sync, Clone, Debug);
assert_not_impl_any!(TaskScheduler: UnwindSafe, RefUnwindSafe);
assert_impl_all!(JoinHandle<()>: Future, Send);
assert_impl_all!(LocalJoinHandle<()>: Future);
assert_not_impl_any!(LocalJoinHandle<()>: Send, Sync);
assert_not_impl_any!(LocalTaskScheduler: Send, Sync);
assert_impl_all!(RuntimeOperations: Send, Sync, Clone, Debug, ThreadAware);
assert_impl_all!(WorkerPoolPolicy: Clone, Debug);
assert_impl_all!(ProcessorCount: Clone, Copy, Debug, Default);
assert_impl_all!(Error: StdError, Send, Sync);
assert_impl_all!(BuildError: StdError, Send, Sync);
assert_impl_all!(Error: From<BuildError>);
assert_not_impl_any!(Error: From<io::Error>, From<Box<dyn StdError + Send + Sync>>);
assert_type_eq_all!(arty::rt::Result<()>, std::result::Result<(), Error>);

const _: fn(NonZeroUsize) -> ProcessorCount = ProcessorCount::exactly;
const _: fn(NonZeroUsize) -> ProcessorCount = ProcessorCount::at_most;

const _: fn(&Runtime) -> TaskScheduler = Runtime::task_scheduler;
