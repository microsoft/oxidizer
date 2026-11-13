// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use thread_aware::ThreadAware;

use super::hook_fn::{ErasedHookFn, HookFn};
use super::{Hook, Panic};

#[derive(Clone, derive_more::Debug, ThreadAware)]
pub(crate) enum HookRegistration {
    Panic(ErasedHookFn<Panic>),
}

impl From<ErasedHookFn<Panic>> for HookRegistration {
    fn from(hook: ErasedHookFn<Panic>) -> Self {
        Self::Panic(hook)
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
