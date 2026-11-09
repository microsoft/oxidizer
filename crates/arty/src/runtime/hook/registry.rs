// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::{Any, TypeId};
use std::task::Waker;

use performables::sync::channel;
use thread_aware::{Thread, ThreadAware};

use super::on_panic::PanicHook;
use super::on_start::StartHook;
use super::{Hook, OnPanic, OnStart};
use crate::runtime::worker::asynchronous::Command;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PanicAction {
    Ignore,
    Shutdown,
}

trait Callback<E>: Send {
    fn clone_box(&self) -> Box<dyn Callback<E>>;
    fn relocate(&mut self, source: Option<&Thread>, destination: &Thread);
    fn execute(&mut self, event: &mut E);
}

struct CallbackWithData<E, D> {
    data: D,
    callback: fn(&mut E, &mut D),
}

impl<E, D> Callback<E> for CallbackWithData<E, D>
where
    E: 'static,
    D: ThreadAware + Clone + 'static,
{
    fn clone_box(&self) -> Box<dyn Callback<E>> {
        Box::new(Self {
            data: self.data.clone(),
            callback: self.callback,
        })
    }

    fn relocate(&mut self, source: Option<&Thread>, destination: &Thread) {
        self.data.relocate(source, destination);
    }

    fn execute(&mut self, event: &mut E) {
        (self.callback)(event, &mut self.data);
    }
}

#[derive(derive_more::Debug)]
pub(crate) struct TypedHook<E> {
    #[debug(ignore)]
    callback: Box<dyn Callback<E>>,
}

impl<E: 'static> TypedHook<E> {
    pub(crate) fn new<D>(data: D, callback: fn(&mut E, &mut D)) -> Self
    where
        D: ThreadAware + Clone + 'static,
    {
        Self {
            callback: Box::new(CallbackWithData { data, callback }),
        }
    }

    pub(crate) fn execute(&mut self, event: &mut E) {
        self.callback.execute(event);
    }
}

impl<E: 'static> Clone for TypedHook<E> {
    fn clone(&self) -> Self {
        Self {
            callback: self.callback.clone_box(),
        }
    }
}

impl<E: 'static> ThreadAware for TypedHook<E> {
    fn relocate(&mut self, source: Option<&Thread>, destination: &Thread) {
        self.callback.relocate(source, destination);
    }
}

#[derive(Clone, Copy, Debug)]
enum HookType {
    Start,
    Panic,
}

struct HookWithData<H, D> {
    data: D,
    callback: fn(&mut H, &mut D),
}

struct CallbackData<H>(fn(&mut H));

impl<H> Copy for CallbackData<H> {}

impl<H> Clone for CallbackData<H> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<H> ThreadAware for CallbackData<H> {
    fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
}

fn execute_callback<H>(event: &mut H, callback: &mut CallbackData<H>) {
    (callback.0)(event);
}

#[derive(Clone, derive_more::Debug)]
pub(crate) enum HookRegistration {
    Start(StartHook),
    Panic(PanicHook),
}

impl HookRegistration {
    pub(crate) fn callback<H: Hook>(callback: fn(&mut H)) -> Self {
        Self::new(CallbackData(callback), execute_callback::<H>)
    }

    pub(crate) fn new<H, D>(data: D, callback: fn(&mut H, &mut D)) -> Self
    where
        H: Hook,
        D: ThreadAware + Clone + 'static,
    {
        let hook: Box<dyn Any + Send> = Box::new(HookWithData { data, callback });
        match hook_type::<H>() {
            HookType::Start => {
                let hook = *hook
                    .downcast::<HookWithData<OnStart, D>>()
                    .expect("OnStart hook type is selected by its TypeId");
                Self::Start(StartHook::new(hook.data, hook.callback))
            }
            HookType::Panic => {
                let hook = *hook
                    .downcast::<HookWithData<OnPanic, D>>()
                    .expect("OnPanic hook type is selected by its TypeId");
                Self::Panic(PanicHook::new(hook.data, hook.callback))
            }
        }
    }

    fn relocate(&mut self, source: Option<&Thread>, destination: &Thread) {
        match self {
            Self::Start(hook) => hook.relocate(source, destination),
            Self::Panic(hook) => hook.relocate(source, destination),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Hooks {
    start: Vec<StartHook>,
    panic: Vec<PanicHook>,
}

impl Hooks {
    pub(crate) fn set_callback<H: Hook>(&mut self, callback: fn(&mut H)) {
        self.push(HookRegistration::callback(callback));
    }

    pub(crate) fn add<H, D>(&mut self, data: D, callback: fn(&mut H, &mut D))
    where
        H: Hook,
        D: ThreadAware + Clone + 'static,
    {
        self.push(HookRegistration::new(data, callback));
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

#[derive(Clone, Debug)]
pub(crate) struct BlockingPanicHook {
    command_tx: Option<channel::Sender<Command>>,
    waker: Waker,
    fixed_action: Option<PanicAction>,
}

impl BlockingPanicHook {
    pub(crate) fn new(command_tx: channel::Sender<Command>, waker: Waker) -> Self {
        Self {
            command_tx: Some(command_tx),
            waker,
            fixed_action: None,
        }
    }

    pub(crate) fn notify(&self) -> PanicAction {
        if let Some(action) = self.fixed_action {
            self.waker.wake_by_ref();
            return action;
        }
        let (ready, action) = std::sync::mpsc::channel();
        if self
            .command_tx
            .as_ref()
            .expect("production blocking panic hooks retain their worker command sender")
            .send(Command::BlockingPanic { ready })
            .is_err()
        {
            return PanicAction::Shutdown;
        }
        self.waker.wake_by_ref();
        action.recv().unwrap_or(PanicAction::Shutdown)
    }

    #[cfg(test)]
    pub(crate) fn for_test(action: PanicAction) -> Self {
        Self::for_test_with_waker(action, Waker::noop().clone())
    }

    #[cfg(test)]
    pub(crate) fn for_test_with_waker(action: PanicAction, waker: Waker) -> Self {
        Self {
            command_tx: None,
            waker,
            fixed_action: Some(action),
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
