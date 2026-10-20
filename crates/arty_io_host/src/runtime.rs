// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::{Any, TypeId};
use std::fmt;
use std::marker::PhantomData;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::task::Waker;

use arty_io_core::{DriverError, DriverRole, IoContext};
use foldhash::HashMap;
use observed::Sink;
use thread_aware_core::{Thread, ThreadAware};

use crate::DriverHost;

pub(crate) struct Resolution<C>
where
    C: IoContext,
{
    pub(crate) context: C,
    pub(crate) role: DriverRole,
    pub(crate) registration: Registration<C>,
}

pub(crate) enum Registration<C>
where
    C: IoContext,
{
    Existing(IoRuntime),
    Pending(PendingRegistration<C>),
}

impl<C> Registration<C>
where
    C: IoContext,
{
    pub(crate) fn commit(self) -> Result<(), DriverError> {
        match self {
            Self::Existing(runtime) => runtime.check_failed(),
            Self::Pending(pending) => pending.commit(),
        }
    }

    pub(crate) fn fail(self, error: &DriverError) {
        match self {
            Self::Existing(runtime) => runtime.fail(error),
            Self::Pending(pending) => pending.fail(error),
        }
    }
}

trait ErasedContext: Send {
    fn clone_context(&self) -> Box<dyn Any>;
}

struct NeutralContext<C>(C);

impl<C> ErasedContext for NeutralContext<C>
where
    C: IoContext,
{
    fn clone_context(&self) -> Box<dyn Any> {
        Box::new(self.0.clone())
    }
}

struct RegisteredContext {
    context: Mutex<Box<dyn ErasedContext>>,
}

impl RegisteredContext {
    fn clone_context<C>(&self) -> C
    where
        C: IoContext,
    {
        *self
            .context
            .lock()
            .expect("a poisoned I/O context indicates a runtime invariant failure")
            .clone_context()
            .downcast::<C>()
            .expect("I/O contexts are stored under their own TypeId")
    }
}

enum ContextState {
    Initializing,
    Ready(Arc<RegisteredContext>),
    Failed(Arc<str>),
}

struct ContextSlot {
    state: Mutex<ContextState>,
    changed: Condvar,
    role: DriverRole,
}

impl ContextSlot {
    fn new(role: DriverRole) -> Self {
        Self {
            state: Mutex::new(ContextState::Initializing),
            changed: Condvar::new(),
            role,
        }
    }

    fn context<C>(&self) -> Result<C, DriverError>
    where
        C: IoContext,
    {
        let registered = {
            let mut state = self.state();
            loop {
                match &*state {
                    ContextState::Initializing => {
                        state = self
                            .changed
                            .wait(state)
                            .expect("a poisoned I/O context slot indicates a runtime invariant failure");
                    }
                    ContextState::Ready(context) => break Arc::clone(context),
                    ContextState::Failed(message) => return Err(DriverError::from_message(message.as_ref())),
                }
            }
        };
        Ok(registered.clone_context::<C>())
    }

    fn publish(&self, context: RegisteredContext) {
        let mut state = self.state();
        assert!(
            matches!(*state, ContextState::Initializing),
            "only an initializing I/O context can be published"
        );
        *state = ContextState::Ready(Arc::new(context));
        drop(state);
        self.changed.notify_all();
    }

    fn fail(&self, message: Arc<str>) {
        let mut state = self.state();
        let previous = std::mem::replace(&mut *state, ContextState::Failed(message));
        drop(state);
        self.changed.notify_all();
        drop(previous);
    }

    fn state(&self) -> MutexGuard<'_, ContextState> {
        self.state
            .lock()
            .expect("a poisoned I/O context slot indicates a runtime invariant failure")
    }
}

#[derive(Default)]
struct Registry {
    contexts: HashMap<TypeId, Arc<ContextSlot>>,
    primary_context: Option<TypeId>,
    failure: Option<Arc<str>>,
}

impl Registry {
    fn check(&self) -> Result<(), DriverError> {
        match &self.failure {
            Some(message) => Err(DriverError::from_message(message.as_ref())),
            None => Ok(()),
        }
    }
}

pub(crate) struct PendingRegistration<C> {
    runtime: IoRuntime,
    context_type: TypeId,
    slot: Arc<ContextSlot>,
    context: Option<C>,
    completed: bool,
    _context: PhantomData<fn() -> C>,
}

impl<C> PendingRegistration<C>
where
    C: IoContext,
{
    fn commit(mut self) -> Result<(), DriverError> {
        let context = self
            .context
            .take()
            .expect("a pending I/O registration retains its neutral context until commit");
        self.runtime.commit_registration(
            self.context_type,
            &self.slot,
            RegisteredContext {
                context: Mutex::new(Box::new(NeutralContext(context))),
            },
        )?;
        self.completed = true;
        Ok(())
    }

    fn fail(mut self, error: &DriverError) {
        self.runtime.fail(error);
        self.completed = true;
    }
}

impl<C> Drop for PendingRegistration<C> {
    fn drop(&mut self) {
        if self.completed {
            return;
        }
        self.runtime.cancel_registration(
            self.context_type,
            &self.slot,
            Arc::from("I/O context initialization was interrupted before publication"),
        );
    }
}

/// Shared registry of thread-neutral I/O contexts for one Arty runtime.
///
/// This type is unstable Arty implementation infrastructure. Applications and
/// libraries should use `arty` instead.
///
/// The first registered context type is assigned the primary role. Later
/// context types are assigned the secondary role. Each worker-local
/// [`DriverHost`](crate::DriverHost) creates drivers lazily when it first
/// accesses a registered context type.
#[derive(Clone)]
pub struct IoRuntime {
    registry: Arc<Mutex<Registry>>,
    sink: Sink,
    #[cfg(test)]
    resolution_hook: Option<Arc<dyn Fn(TypeId) + Send + Sync>>,
}

impl fmt::Debug for IoRuntime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IoRuntime").finish_non_exhaustive()
    }
}

impl PartialEq for IoRuntime {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.registry, &other.registry)
    }
}

impl Eq for IoRuntime {}

impl IoRuntime {
    /// Creates an empty runtime-wide I/O context registry.
    #[must_use]
    pub fn new(sink: Sink) -> Self {
        Self {
            registry: Arc::new(Mutex::new(Registry::default())),
            sink,
            #[cfg(test)]
            resolution_hook: None,
        }
    }

    /// Creates a worker-local driver host for one Arty async worker.
    ///
    /// The supplied thread must identify the worker calling this method. The
    /// runtime relocates its worker-local telemetry state to that thread before
    /// constructing the host.
    ///
    /// # Errors
    ///
    /// Returns an error if `thread` does not identify the current thread.
    pub fn driver_host(mut self, thread: Thread, owner_waker: Waker) -> Result<DriverHost, DriverError> {
        if thread.id() != std::thread::current().id() {
            return Err(DriverError::from_message(
                "I/O driver host must be created on the worker identified by its thread coordinate",
            ));
        }
        self.sink.relocate(None, &thread);
        Ok(DriverHost::new(self, thread, owner_waker))
    }

    pub(crate) fn resolve_context<C>(&self, options: C::ContextOptions) -> Result<Resolution<C>, DriverError>
    where
        C: IoContext,
    {
        let context_type = TypeId::of::<C>();
        let (slot, is_new) = {
            let mut registry = self.registry();
            registry.check()?;
            if let Some(slot) = registry.contexts.get(&context_type) {
                (Arc::clone(slot), false)
            } else {
                let role = if registry.primary_context.is_none() {
                    registry.primary_context = Some(context_type);
                    DriverRole::Primary
                } else {
                    DriverRole::Secondary
                };
                let slot = Arc::new(ContextSlot::new(role));
                registry.contexts.insert(context_type, Arc::clone(&slot));
                (slot, true)
            }
        };
        #[cfg(test)]
        if let Some(hook) = &self.resolution_hook {
            hook(context_type);
        }
        if !is_new {
            return Ok(Resolution {
                context: slot.context::<C>()?,
                role: slot.role,
                registration: Registration::Existing(self.clone()),
            });
        }

        let mut pending = PendingRegistration {
            runtime: self.clone(),
            context_type,
            slot,
            context: None,
            completed: false,
            _context: PhantomData,
        };
        let context = C::context(options);
        let local_context = context.clone();
        pending.context = Some(context);
        Ok(Resolution {
            context: local_context,
            role: pending.slot.role,
            registration: Registration::Pending(pending),
        })
    }

    pub(crate) fn check_failed(&self) -> Result<(), DriverError> {
        self.registry().check()
    }

    pub(crate) fn fail(&self, error: &DriverError) {
        let message: Arc<str> = Arc::from(error.to_string());
        let slots = {
            let mut registry = self.registry();
            registry.failure.get_or_insert_with(|| Arc::clone(&message));
            registry.primary_context = None;
            registry.contexts.drain().map(|(_, slot)| slot).collect::<Vec<_>>()
        };
        for slot in slots {
            slot.fail(Arc::clone(&message));
        }
    }

    pub(crate) const fn sink(&self) -> &Sink {
        &self.sink
    }

    #[cfg(test)]
    fn set_resolution_hook(&mut self, hook: Arc<dyn Fn(TypeId) + Send + Sync>) {
        self.resolution_hook = Some(hook);
    }

    fn registry(&self) -> MutexGuard<'_, Registry> {
        self.registry
            .lock()
            .expect("a poisoned I/O context registry indicates a runtime invariant failure")
    }

    fn cancel_registration(&self, context_type: TypeId, slot: &Arc<ContextSlot>, message: Arc<str>) {
        let removed = {
            let mut registry = self.registry();
            let matches = registry
                .contexts
                .get(&context_type)
                .is_some_and(|registered| Arc::ptr_eq(registered, slot));
            if matches {
                registry.contexts.remove(&context_type);
                if registry.primary_context == Some(context_type) {
                    registry.primary_context = None;
                }
                true
            } else {
                false
            }
        };
        if removed {
            slot.fail(message);
        }
    }

    fn commit_registration(&self, context_type: TypeId, slot: &Arc<ContextSlot>, context: RegisteredContext) -> Result<(), DriverError> {
        let registry = self.registry();
        registry.check()?;
        assert!(
            registry
                .contexts
                .get(&context_type)
                .is_some_and(|registered| Arc::ptr_eq(registered, slot)),
            "a pending I/O registration must remain reserved until commit"
        );
        slot.publish(context);
        Ok(())
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::any::TypeId;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Barrier};

    use arty_io_core::{Driver, DriverError, DriverOptions, IoContext};
    use observed::Sink;
    use thread_aware_core::{Thread, ThreadAware};

    use super::IoRuntime;

    static CONTEXT_CALLS: AtomicUsize = AtomicUsize::new(0);

    #[derive(Clone)]
    struct TestContext;

    impl ThreadAware for TestContext {
        fn relocate(&mut self, _source: Option<&Thread>, _destination: &Thread) {}
    }

    impl IoContext for TestContext {
        type ContextOptions = ();
        type Driver = TestDriver;

        fn context((): Self::ContextOptions) -> Self {
            CONTEXT_CALLS.fetch_add(1, Ordering::SeqCst);
            Self
        }

        fn driver(&mut self, _options: DriverOptions) -> Result<Self::Driver, DriverError> {
            Ok(TestDriver)
        }
    }

    struct TestDriver;

    impl Driver for TestDriver {
        fn waker(&self) -> std::task::Waker {
            std::task::Waker::noop().clone()
        }

        fn execute_cycle(&mut self, _cycle: &mut arty_io_core::Cycle) -> Result<(), DriverError> {
            Ok(())
        }

        fn shutdown(self) -> Result<(), DriverError> {
            Ok(())
        }
    }

    #[test]
    fn concurrent_first_retrieval_initializes_once() {
        CONTEXT_CALLS.store(0, Ordering::SeqCst);
        let barrier = Arc::new(Barrier::new(3));
        let hook_barrier = Arc::clone(&barrier);
        let mut runtime = IoRuntime::new(Sink::noop());
        runtime.set_resolution_hook(Arc::new(move |context_type| {
            assert_eq!(context_type, TypeId::of::<TestContext>());
            hook_barrier.wait();
        }));

        let first_runtime = runtime.clone();
        let first = std::thread::spawn(move || {
            let resolution = first_runtime.resolve_context::<TestContext>(()).unwrap();
            resolution.registration.commit().unwrap();
        });
        let second = std::thread::spawn(move || {
            let resolution = runtime.resolve_context::<TestContext>(()).unwrap();
            resolution.registration.commit().unwrap();
        });
        barrier.wait();

        first.join().unwrap();
        second.join().unwrap();

        assert_eq!(CONTEXT_CALLS.load(Ordering::SeqCst), 1);
    }
}
