// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::{Any, TypeId};
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
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
    pub(crate) initialization: DriverInitializationGuard,
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

struct ContextSlot {
    context: OnceLock<RegisteredContext>,
    initializing_threads: Mutex<Vec<std::thread::ThreadId>>,
    role: DriverRole,
}

impl ContextSlot {
    fn new(role: DriverRole) -> Self {
        Self {
            context: OnceLock::new(),
            initializing_threads: Mutex::new(Vec::new()),
            role,
        }
    }

    fn begin_initialization(self: &Arc<Self>) -> Result<DriverInitializationGuard, DriverError> {
        let thread = std::thread::current().id();
        let mut initializing_threads = self
            .initializing_threads
            .lock()
            .expect("a poisoned I/O context initializer indicates a runtime invariant failure");
        if initializing_threads.contains(&thread) {
            return Err(DriverError::from_message(
                "I/O context retrieval cannot reenter initialization of the same context type on one thread",
            ));
        }
        initializing_threads.push(thread);
        drop(initializing_threads);
        Ok(DriverInitializationGuard {
            slot: Arc::clone(self),
            thread,
        })
    }

    fn resolve<C>(&self, options: C::ContextOptions) -> C
    where
        C: IoContext,
    {
        let registered = self.context.get_or_init(|| RegisteredContext {
            context: Mutex::new(Box::new(NeutralContext(C::context(options)))),
        });
        registered.clone_context::<C>()
    }
}

pub(crate) struct DriverInitializationGuard {
    slot: Arc<ContextSlot>,
    thread: std::thread::ThreadId,
}

impl Drop for DriverInitializationGuard {
    fn drop(&mut self) {
        let mut initializing_threads = self
            .slot
            .initializing_threads
            .lock()
            .expect("a poisoned I/O context initializer indicates a runtime invariant failure");
        let index = initializing_threads
            .iter()
            .position(|thread| *thread == self.thread)
            .expect("an I/O initialization guard removes its registered thread");
        initializing_threads.swap_remove(index);
    }
}

#[derive(Default)]
struct Registry {
    contexts: HashMap<TypeId, Arc<ContextSlot>>,
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
        let slot = {
            let mut registry = self.registry();
            registry.check()?;
            let primary_available = registry.contexts.is_empty();
            Arc::clone(registry.contexts.entry(context_type).or_insert_with(|| {
                Arc::new(ContextSlot::new(if primary_available {
                    DriverRole::Primary
                } else {
                    DriverRole::Secondary
                }))
            }))
        };
        #[cfg(test)]
        if let Some(hook) = &self.resolution_hook {
            hook(context_type);
        }
        let initialization = slot.begin_initialization()?;
        Ok(Resolution {
            context: slot.resolve::<C>(options),
            role: slot.role,
            initialization,
        })
    }

    pub(crate) fn check_failed(&self) -> Result<(), DriverError> {
        self.registry().check()
    }

    pub(crate) fn fail(&self, error: &DriverError) {
        let message: Arc<str> = Arc::from(error.to_string());
        self.registry().failure.get_or_insert(message);
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
        let first = std::thread::spawn(move || first_runtime.resolve_context::<TestContext>(()).unwrap());
        let second = std::thread::spawn(move || runtime.resolve_context::<TestContext>(()).unwrap());
        barrier.wait();

        let _ = first.join().unwrap();
        let _ = second.join().unwrap();

        assert_eq!(CONTEXT_CALLS.load(Ordering::SeqCst), 1);
    }
}
