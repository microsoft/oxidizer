// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::marker::PhantomData;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::task::Waker;

use arty_executor::ShutdownCause;
use performables::sync::channel;
use thread_aware::{Thread, ThreadAware};

use super::{Hook, OnPanic, OnSpawn, OnStart};
use crate::runtime::Shutdown;
use crate::runtime::shutdown::ShutdownState;
use crate::runtime::worker::asynchronous::Command;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum PanicAction {
    Ignore = 0,
    Shutdown = 1,
}

impl PanicAction {
    fn from_u8(value: u8) -> Self {
        match value {
            0 => Self::Ignore,
            _ => Self::Shutdown,
        }
    }
}

trait Callback: Send {
    fn relocate(&mut self, source: Option<&Thread>, destination: &Thread);
    fn invoke(&mut self, thread: &Thread) -> HookOutcome;
}

struct TypedCallback<H, D> {
    data: D,
    callback: fn(&mut H, &mut D),
}

struct CallbackOnly<H> {
    callback: fn(&mut H),
}

impl<H> Callback for CallbackOnly<H>
where
    H: Hook,
{
    fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}

    fn invoke(&mut self, thread: &Thread) -> HookOutcome {
        fn invoke<H>(event: &mut H, callback: &mut fn(&mut H)) {
            callback(event);
        }

        let mut callback = self.callback;
        let mut handlers = Vec::new();
        let action = <H as private::Sealed>::invoke(invoke::<H>, &mut callback, thread, &mut handlers);
        outcome::<H>(action, handlers)
    }
}

impl<H, D> Callback for TypedCallback<H, D>
where
    H: Hook,
    D: ThreadAware + 'static,
{
    fn relocate(&mut self, source: Option<&Thread>, destination: &Thread) {
        self.data.relocate(source, destination);
    }

    fn invoke(&mut self, thread: &Thread) -> HookOutcome {
        let mut handlers = Vec::new();
        let action = <H as private::Sealed>::invoke(self.callback, &mut self.data, thread, &mut handlers);
        outcome::<H>(action, handlers)
    }
}

trait CallbackFactory: Send + Sync {
    fn make(&self) -> Box<dyn Callback>;
}

struct TypedCallbackFactory<H, D> {
    data: Mutex<D>,
    callback: fn(&mut H, &mut D),
    marker: PhantomData<fn(H)>,
}

struct CallbackOnlyFactory<H> {
    callback: fn(&mut H),
}

impl<H> CallbackFactory for CallbackOnlyFactory<H>
where
    H: Hook,
{
    fn make(&self) -> Box<dyn Callback> {
        Box::new(CallbackOnly::<H> { callback: self.callback })
    }
}

impl<H, D> CallbackFactory for TypedCallbackFactory<H, D>
where
    H: Hook,
    D: ThreadAware + Clone + 'static,
{
    fn make(&self) -> Box<dyn Callback> {
        Box::new(TypedCallback::<H, D> {
            data: self
                .data
                .lock()
                .expect("hook data is only locked while Arty clones it during registration")
                .clone(),
            callback: self.callback,
        })
    }
}

#[derive(Clone, Copy, Debug)]
enum HookKind {
    Start,
    Spawn,
    Panic,
}

#[derive(derive_more::Debug)]
pub(crate) struct HookFactory {
    kind: HookKind,
    #[debug(ignore)]
    callback: Box<dyn CallbackFactory>,
}

impl HookFactory {
    pub(crate) fn without_data<H: Hook>(callback: fn(&mut H)) -> Self {
        Self {
            kind: hook_kind::<H>(),
            callback: Box::new(CallbackOnlyFactory::<H> { callback }),
        }
    }

    pub(crate) fn new<H, D>(data: D, callback: fn(&mut H, &mut D)) -> Self
    where
        H: Hook,
        D: ThreadAware + Clone + 'static,
    {
        Self {
            kind: hook_kind::<H>(),
            callback: Box::new(TypedCallbackFactory::<H, D> {
                data: Mutex::new(data),
                callback,
                marker: PhantomData,
            }),
        }
    }

    pub(crate) fn make(&self) -> WorkerHook {
        WorkerHook {
            kind: self.kind,
            callback: self.callback.make(),
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct HookFactories {
    start: Option<HookFactory>,
    spawn: Option<HookFactory>,
    panic: Option<HookFactory>,
}

impl HookFactories {
    pub(crate) fn set_without_data<H: Hook>(&mut self, callback: fn(&mut H)) {
        self.set_factory(HookFactory::without_data(callback));
    }

    pub(crate) fn set<H, D>(&mut self, data: D, callback: fn(&mut H, &mut D))
    where
        H: Hook,
        D: ThreadAware + Clone + 'static,
    {
        self.set_factory(HookFactory::new(data, callback));
    }

    fn set_factory(&mut self, factory: HookFactory) {
        match factory.kind {
            HookKind::Start => self.start = Some(factory),
            HookKind::Spawn => self.spawn = Some(factory),
            HookKind::Panic => self.panic = Some(factory),
        }
    }

    pub(crate) fn seed(&self) -> WorkerHookSeed {
        WorkerHookSeed {
            start: self.start.as_ref().map(HookFactory::make),
            spawn: self.spawn.as_ref().map(HookFactory::make),
            panic: self.panic.as_ref().map(HookFactory::make),
        }
    }
}

fn hook_kind<H: Hook>() -> HookKind {
    match <H as private::Sealed>::KIND {
        private::START => HookKind::Start,
        private::SPAWN => HookKind::Spawn,
        private::PANIC => HookKind::Panic,
        _ => unreachable!("sealed hook kinds are defined by Arty"),
    }
}

fn outcome<H: Hook>(action: u8, handlers: Vec<Box<dyn arty_executor::TaskPoll>>) -> HookOutcome {
    match <H as private::Sealed>::KIND {
        private::START => HookOutcome::None,
        private::SPAWN => HookOutcome::Spawn(handlers),
        private::PANIC => HookOutcome::Panic(PanicAction::from_u8(action)),
        _ => unreachable!("sealed hook kinds are defined by Arty"),
    }
}

#[derive(derive_more::Debug)]
pub(crate) struct WorkerHook {
    kind: HookKind,
    #[debug(ignore)]
    callback: Box<dyn Callback>,
}

#[derive(Debug)]
pub(crate) struct WorkerHookSeed {
    start: Option<WorkerHook>,
    spawn: Option<WorkerHook>,
    panic: Option<WorkerHook>,
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

#[derive(derive_more::Debug)]
pub(crate) struct WorkerHooks {
    #[debug(ignore)]
    start: Option<Box<dyn Callback>>,
    #[debug(ignore)]
    spawn: Option<Box<dyn Callback>>,
    #[debug(ignore)]
    panic: Option<Box<dyn Callback>>,
    shutdown: Shutdown,
    thread: Thread,
}

impl WorkerHooks {
    pub(crate) fn new(seed: WorkerHookSeed, thread: Thread, shutdown: Shutdown) -> Self {
        let mut hooks = Self {
            start: None,
            spawn: None,
            panic: None,
            shutdown,
            thread,
        };
        if let Some(hook) = seed.start {
            hooks.install(hook, None);
        }
        if let Some(hook) = seed.spawn {
            hooks.install(hook, None);
        }
        if let Some(hook) = seed.panic {
            hooks.install(hook, None);
        }
        hooks
    }

    pub(crate) fn install(&mut self, mut hook: WorkerHook, source: Option<&Thread>) {
        hook.callback.relocate(source, &self.thread);
        match hook.kind {
            HookKind::Start => self.start = Some(hook.callback),
            HookKind::Spawn => self.spawn = Some(hook.callback),
            HookKind::Panic => self.panic = Some(hook.callback),
        }
    }

    pub(crate) fn on_start(&mut self) {
        if let Some(callback) = self.start.as_mut() {
            _ = callback.invoke(&self.thread);
        }
    }

    pub(crate) fn on_spawn(&mut self) -> Option<Vec<Box<dyn arty_executor::TaskPoll>>> {
        let handlers = if let Some(callback) = self.spawn.as_mut() {
            match callback.invoke(&self.thread) {
                HookOutcome::Spawn(handlers) => handlers,
                HookOutcome::None | HookOutcome::Panic(_) => {
                    unreachable!("OnSpawn callbacks return poll handlers")
                }
            }
        } else {
            Vec::new()
        };
        if matches!(self.shutdown.state(), ShutdownState::Running) {
            Some(handlers)
        } else {
            None
        }
    }

    pub(crate) fn on_panic(&mut self) -> PanicAction {
        let action = self
            .panic
            .as_mut()
            .map_or(PanicAction::Shutdown, |callback| match callback.invoke(&self.thread) {
                HookOutcome::Panic(action) => action,
                HookOutcome::None | HookOutcome::Spawn(_) => {
                    unreachable!("OnPanic callbacks return a panic action")
                }
            });
        if action == PanicAction::Shutdown {
            self.shutdown.begin(ShutdownCause::panic());
        }
        action
    }
}

pub(super) mod private {
    use super::*;

    pub(super) const START: u8 = 0;
    pub(super) const SPAWN: u8 = 1;
    pub(super) const PANIC: u8 = 2;

    pub trait Sealed: Sized {
        const KIND: u8;

        fn invoke<D>(
            callback: fn(&mut Self, &mut D),
            data: &mut D,
            thread: &Thread,
            handlers: &mut Vec<Box<dyn arty_executor::TaskPoll>>,
        ) -> u8;
    }

    impl Sealed for OnStart {
        const KIND: u8 = START;

        fn invoke<D>(
            callback: fn(&mut Self, &mut D),
            data: &mut D,
            _thread: &Thread,
            _handlers: &mut Vec<Box<dyn arty_executor::TaskPoll>>,
        ) -> u8 {
            callback(&mut Self { thread: _thread.clone() }, data);
            PanicAction::Ignore as u8
        }
    }

    impl Sealed for OnSpawn {
        const KIND: u8 = SPAWN;

        fn invoke<D>(
            callback: fn(&mut Self, &mut D),
            data: &mut D,
            thread: &Thread,
            handlers: &mut Vec<Box<dyn arty_executor::TaskPoll>>,
        ) -> u8 {
            let mut event = Self {
                thread: thread.clone(),
                handlers: Vec::new(),
            };
            callback(&mut event, data);
            handlers.append(&mut event.handlers);
            PanicAction::Ignore as u8
        }
    }

    impl Sealed for OnPanic {
        const KIND: u8 = PANIC;

        fn invoke<D>(
            callback: fn(&mut Self, &mut D),
            data: &mut D,
            _thread: &Thread,
            _handlers: &mut Vec<Box<dyn arty_executor::TaskPoll>>,
        ) -> u8 {
            let action = Arc::new(AtomicU8::new(PanicAction::Shutdown as u8));
            callback(
                &mut Self {
                    action: Arc::clone(&action),
                },
                data,
            );
            action.load(Ordering::Acquire)
        }
    }
}

enum HookOutcome {
    None,
    Spawn(Vec<Box<dyn arty_executor::TaskPoll>>),
    Panic(PanicAction),
}
