// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use thread_aware::ThreadAware;

use super::hook_fn::{ErasedHookFn, HookFn};
use super::{Hook, TaskPanic, TaskSpawn, WorkerSpawn};

#[derive(Clone, derive_more::Debug, ThreadAware)]
pub(crate) enum HookRegistration {
    TaskSpawn(ErasedHookFn<TaskSpawn>),
    TaskPanic(ErasedHookFn<TaskPanic>),
    WorkerSpawn(ErasedHookFn<WorkerSpawn>),
}

pub(crate) trait RegisterHook<D>: Hook
where
    D: ThreadAware + Clone + 'static,
{
    fn register(hook: HookFn<Self, D>) -> HookRegistration;
}

impl HookRegistration {
    pub(crate) const fn is_retained(&self) -> bool {
        matches!(self, Self::TaskSpawn(_) | Self::WorkerSpawn(_))
    }
}

impl From<ErasedHookFn<TaskPanic>> for HookRegistration {
    fn from(hook: ErasedHookFn<TaskPanic>) -> Self {
        Self::TaskPanic(hook)
    }
}

impl From<ErasedHookFn<WorkerSpawn>> for HookRegistration {
    fn from(hook: ErasedHookFn<WorkerSpawn>) -> Self {
        Self::WorkerSpawn(hook)
    }
}

impl<D> RegisterHook<D> for TaskPanic
where
    D: ThreadAware + Clone + 'static,
{
    fn register(hook: HookFn<Self, D>) -> HookRegistration {
        ErasedHookFn::<Self>::from(hook).into()
    }
}

impl<D> RegisterHook<D> for WorkerSpawn
where
    D: ThreadAware + Clone + 'static,
{
    fn register(hook: HookFn<Self, D>) -> HookRegistration {
        ErasedHookFn::<Self>::from(hook).into()
    }
}

impl<D> RegisterHook<D> for TaskSpawn
where
    D: ThreadAware + Clone + Sync + 'static,
{
    fn register(hook: HookFn<Self, D>) -> HookRegistration {
        ErasedHookFn::<Self>::from(hook).into()
    }
}

pub(crate) fn registration<H, D>(hook: HookFn<H, D>) -> HookRegistration
where
    H: Hook + RegisterHook<D>,
    D: ThreadAware + Clone + 'static,
{
    H::register(hook)
}
