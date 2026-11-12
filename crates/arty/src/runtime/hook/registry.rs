// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::{Any, TypeId};

use thread_aware::{Thread, ThreadAware};

use super::on_panic::PanicHook;
use super::on_start::StartHook;
use super::typed::{HookFn, TypedHook};
use super::{Hook, OnPanic, OnStart};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PanicAction {
    Ignore,
    Shutdown,
}

#[derive(Clone, Copy, Debug)]
enum HookType {
    Start,
    Panic,
}

#[derive(Clone, derive_more::Debug, ThreadAware)]
pub(crate) enum HookRegistration {
    Start(StartHook),
    Panic(PanicHook),
}

impl HookRegistration {
    pub(crate) fn new<H, D>(hook: HookFn<H, D>) -> Self
    where
        H: Hook,
        D: ThreadAware + Clone + 'static,
    {
        let hook: Box<dyn Any + Send> = Box::new(hook.erased());
        match hook_type::<H>() {
            HookType::Start => {
                let hook = *hook
                    .downcast::<TypedHook<OnStart>>()
                    .expect("OnStart hook type is selected by its TypeId");
                Self::Start(StartHook::new(hook))
            }
            HookType::Panic => {
                let hook = *hook
                    .downcast::<TypedHook<OnPanic>>()
                    .expect("OnPanic hook type is selected by its TypeId");
                Self::Panic(PanicHook::new(hook))
            }
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Hooks {
    start: Vec<StartHook>,
    panic: Vec<PanicHook>,
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
        match hook {
            HookRegistration::Start(hook) => self.start.push(hook),
            HookRegistration::Panic(hook) => self.panic.push(hook),
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

fn hook_type<H: Hook>() -> HookType {
    if TypeId::of::<H>() == TypeId::of::<OnStart>() {
        HookType::Start
    } else {
        debug_assert_eq!(TypeId::of::<H>(), TypeId::of::<OnPanic>());
        HookType::Panic
    }
}

pub(super) mod private {
    pub trait Sealed {}

    impl Sealed for super::OnStart {}
    impl Sealed for super::OnPanic {}
}
