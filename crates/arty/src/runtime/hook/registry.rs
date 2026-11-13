// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use thread_aware::ThreadAware;

use super::hook_fn::{ErasedHookFn, HookFn};
use super::{Hook, Panic, WorkerInit};

#[derive(Clone, derive_more::Debug, ThreadAware)]
pub(crate) enum HookRegistration {
    Panic(ErasedHookFn<Panic>),
    WorkerInit(ErasedHookFn<WorkerInit>),
}

impl HookRegistration {
    pub(crate) const fn is_retained(&self) -> bool {
        match self {
            Self::Panic(_) => false,
            Self::WorkerInit(_) => true,
        }
    }
}

impl From<ErasedHookFn<Panic>> for HookRegistration {
    fn from(hook: ErasedHookFn<Panic>) -> Self {
        Self::Panic(hook)
    }
}

impl From<ErasedHookFn<WorkerInit>> for HookRegistration {
    fn from(hook: ErasedHookFn<WorkerInit>) -> Self {
        Self::WorkerInit(hook)
    }
}

pub(crate) fn registration<H, D>(hook: HookFn<H, D>) -> HookRegistration
where
    H: Hook,
    D: ThreadAware + Clone + 'static,
{
    let hook: <H as super::private::Sealed>::Erased<D> = hook.into();
    hook.into()
}
