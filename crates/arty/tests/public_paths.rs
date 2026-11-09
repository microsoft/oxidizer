// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public runtime paths, auto traits, and construction signatures.

#![cfg(feature = "rt")]

use std::error::Error as StdError;
use std::fmt::Debug;
use std::io;
use std::panic::{RefUnwindSafe, UnwindSafe};

use arty::runtime::hook::{Hook, OnPanic, OnSpawn, OnStart};
use arty::runtime::{BlockingPoolPolicy, Error, MonitorPolicy, Runtime, RuntimeBuilder, RuntimeControl, Workers, WorkersPolicy};
use arty::task::{Context, JoinError, JoinHandle, RuntimeScheduler, Scheduler};
use static_assertions::{assert_impl_all, assert_not_impl_any};
use thread_aware::ThreadAware;

assert_impl_all!(Runtime: Send, Sync, Debug);
assert_not_impl_any!(Runtime: ThreadAware);
assert_impl_all!(RuntimeBuilder: Debug);
assert_impl_all!(Context: Send, Sync, Clone, Debug, ThreadAware,
    AsRef<Scheduler>, AsRef<arty::time::Clock>);
assert_impl_all!(Scheduler: Send, Sync, Clone, Debug);
assert_not_impl_any!(Scheduler: UnwindSafe, RefUnwindSafe);
assert_impl_all!(JoinHandle<()>: Future, Send, UnwindSafe, RefUnwindSafe);
assert_impl_all!(JoinError: StdError, Send, Sync, Debug);
assert_impl_all!(RuntimeControl: Send, Sync, Clone, Debug);
assert_not_impl_any!(RuntimeControl: ThreadAware, From<&'static Context>, From<&'static Runtime>);
assert_impl_all!(Workers: Send, Sync, Debug);
assert_not_impl_any!(Workers: Clone);
assert_impl_all!(RuntimeScheduler: Send, Sync, Debug);
assert_not_impl_any!(RuntimeScheduler: Clone, ThreadAware);
assert_impl_all!(BlockingPoolPolicy: Clone, Debug);
assert_impl_all!(MonitorPolicy: Clone, Debug, Default);
assert_not_impl_any!(MonitorPolicy: Copy);
assert_impl_all!(OnStart: Debug, Hook);
assert_impl_all!(OnSpawn: Debug, Hook);
assert_impl_all!(OnPanic: Debug, Hook);
assert_not_impl_any!(OnStart: Copy);
assert_not_impl_any!(OnSpawn: Copy);
assert_not_impl_any!(OnPanic: Copy);
assert_impl_all!(WorkersPolicy: Clone, Copy, Debug, Default);
assert_impl_all!(Error: StdError, Send, Sync);
assert_not_impl_any!(Error: From<io::Error>, From<Box<dyn StdError + Send + Sync>>);

const _: fn(usize) -> WorkersPolicy = WorkersPolicy::exactly;
const _: fn(usize) -> WorkersPolicy = WorkersPolicy::at_most;
const _: fn() -> MonitorPolicy = MonitorPolicy::enabled;
const _: fn() -> MonitorPolicy = MonitorPolicy::disabled;
const _: fn(&OnPanic) = OnPanic::shutdown;
const _: fn(&OnPanic) = OnPanic::ignore;
const _: fn(RuntimeBuilder, fn(&mut OnPanic)) -> RuntimeBuilder = RuntimeBuilder::on::<OnPanic>;
const _: fn(RuntimeBuilder, (), fn(&mut OnPanic, &mut ())) -> RuntimeBuilder = RuntimeBuilder::on_with::<OnPanic, ()>;

const _: fn(&Runtime) -> &RuntimeScheduler = Runtime::scheduler;
const _: fn(Runtime) -> Result<(), Error> = Runtime::stop;
const _: fn(&Context) -> &RuntimeControl = Context::control;
const _: fn(&RuntimeControl) = RuntimeControl::request_stop;
const _: fn(&RuntimeControl) -> &Workers = RuntimeControl::workers;
const _: fn(&RuntimeControl, fn(&mut OnPanic)) -> Result<(), Error> = RuntimeControl::on::<OnPanic>;
const _: fn(&RuntimeControl, (), fn(&mut OnPanic, &mut ())) -> Result<(), Error> = RuntimeControl::on_with::<OnPanic, ()>;
fn assert_join_output<F: Future<Output = Result<u32, JoinError>>>() {}
const _: fn() = assert_join_output::<JoinHandle<u32>>;
