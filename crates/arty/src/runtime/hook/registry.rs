// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use thread_aware::{Thread, ThreadAware};

use super::hook_fn::{ErasedHookFn, HookFn};
use super::{Hook, OnPanic, OnStart};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PanicAction {
    Ignore,
    Shutdown,
}

#[derive(Clone, derive_more::Debug, ThreadAware)]
pub struct HookRegistration(HookRegistrationKind);

#[derive(Clone, derive_more::Debug, ThreadAware)]
enum HookRegistrationKind {
    Start(ErasedHookFn<OnStart>),
    Panic(ErasedHookFn<OnPanic>),
}

impl HookRegistration {
    pub(super) const fn start(hook: ErasedHookFn<OnStart>) -> Self {
        Self(HookRegistrationKind::Start(hook))
    }

    pub(super) const fn panic(hook: ErasedHookFn<OnPanic>) -> Self {
        Self(HookRegistrationKind::Panic(hook))
    }

    pub(crate) fn new<H, D>(hook: HookFn<H, D>) -> Self
    where
        H: Hook,
        D: ThreadAware + Clone + 'static,
    {
        H::into_registration(hook)
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Hooks {
    start: Vec<ErasedHookFn<OnStart>>,
    panic: Vec<ErasedHookFn<OnPanic>>,
}

impl Hooks {
    pub(crate) fn add<H, D>(&mut self, hook: HookFn<H, D>)
    where
        H: Hook,
        D: ThreadAware + Clone + 'static,
    {
        self.push(HookRegistration::new(hook));
    }

    pub(crate) fn install(&mut self, mut hook: HookRegistration, source: Option<&Thread>, destination: &Thread) {
        hook.relocate(source, destination);
        self.push(hook);
    }

    fn push(&mut self, hook: HookRegistration) {
        match hook.0 {
            HookRegistrationKind::Start(hook) => self.start.push(hook),
            HookRegistrationKind::Panic(hook) => self.panic.push(hook),
        }
    }

    pub(crate) fn execute_start(&mut self, event: &mut OnStart) {
        for hook in &mut self.start {
            hook.execute(event);
        }
    }

    pub(crate) fn execute_panic(&mut self, event: &mut OnPanic) {
        for hook in &mut self.panic {
            hook.execute(event);
        }
    }
}

impl ThreadAware for Hooks {
    fn relocate(&mut self, source: Option<&Thread>, destination: &Thread) {
        for hook in &mut self.start {
            hook.relocate(source, destination);
        }
        for hook in &mut self.panic {
            hook.relocate(source, destination);
        }
    }
}
