// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use thread_aware::{Thread, ThreadAware};

use super::hook_fn::{ErasedHookFn, HookFn};
use super::{Hook, Panic};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PanicAction {
    Ignore,
    Shutdown,
}

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

#[derive(Clone, Debug, Default)]
pub(crate) struct Hooks {
    panic: Vec<ErasedHookFn<Panic>>,
}

impl Hooks {
    pub(crate) fn add<H, D>(&mut self, hook: HookFn<H, D>)
    where
        H: Hook,
        D: ThreadAware + Clone + 'static,
    {
        self.push(registration(hook));
    }

    pub(crate) fn install(&mut self, mut hook: HookRegistration, source: Option<&Thread>, destination: &Thread) {
        hook.relocate(source, destination);
        self.push(hook);
    }

    fn push(&mut self, hook: HookRegistration) {
        match hook {
            HookRegistration::Panic(hook) => self.panic.push(hook),
        }
    }

    pub(crate) fn execute_panic(&mut self, event: &mut Panic) {
        for hook in &mut self.panic {
            hook.execute(event);
        }
    }
}

impl ThreadAware for Hooks {
    fn relocate(&mut self, source: Option<&Thread>, destination: &Thread) {
        for hook in &mut self.panic {
            hook.relocate(source, destination);
        }
    }
}
