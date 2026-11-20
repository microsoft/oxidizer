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

pub(crate) fn registration<H, D>(hook: HookFn<H, D>) -> HookRegistration
where
    H: Hook,
    D: ThreadAware + Clone + Sync + 'static,
{
    let hook: <H as super::private::Sealed>::Erased<D> = hook.into();
    hook.into()
}
