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

pub(crate) struct Resolution<C> {
    pub(crate) context: C,
    pub(crate) role: DriverRole,
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
    role: DriverRole,
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
    thread: Option<Thread>,
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

impl ThreadAware for IoRuntime {
    fn relocate(&mut self, source: Option<&Thread>, destination: &Thread) {
        self.sink.relocate(source, destination);
        self.thread = Some(destination.clone());
    }
}

impl IoRuntime {
    /// Creates an empty runtime-wide I/O context registry.
    #[must_use]
    pub fn new(sink: Sink) -> Self {
        Self {
            registry: Arc::new(Mutex::new(Registry::default())),
            sink,
            thread: None,
            #[cfg(test)]
            resolution_hook: None,
        }
    }

    /// Creates a worker-local driver host for one Arty async worker.
    ///
    /// # Panics
    ///
    /// Panics if this runtime clone has not been relocated onto the current
    /// worker thread.
    #[must_use]
    pub fn driver_host(self, owner_waker: Waker) -> DriverHost {
        let thread = self
            .thread
            .clone()
            .expect("IoRuntime must be relocated onto its worker before creating a driver host");
        assert_eq!(
            thread.id(),
            std::thread::current().id(),
            "IoRuntime must create its driver host on the thread it was relocated onto"
        );
        DriverHost::new(self, thread, owner_waker)
    }

    pub(crate) fn resolve_context<C>(&self, options: &C::ContextOptions) -> Result<Resolution<C>, DriverError>
    where
        C: IoContext,
        C::ContextOptions: Clone,
    {
        let context_type = TypeId::of::<C>();
        let slot = {
            let mut registry = self.registry();
            registry.check()?;
            let primary_available = registry.contexts.is_empty();
            Arc::clone(registry.contexts.entry(context_type).or_insert_with(|| {
                Arc::new(ContextSlot {
                    context: OnceLock::new(),
                    role: if primary_available {
                        DriverRole::Primary
                    } else {
                        DriverRole::Secondary
                    },
                })
            }))
        };
        #[cfg(test)]
        if let Some(hook) = &self.resolution_hook {
            hook(context_type);
        }
        let registered = slot.context.get_or_init(|| RegisteredContext {
            context: Mutex::new(Box::new(NeutralContext(C::context(options.clone())))),
        });
        Ok(Resolution {
            context: registered.clone_context::<C>(),
            role: slot.role,
        })
    }

    pub(crate) fn check_failed(&self) -> Result<(), DriverError> {
        self.registry().check()
    }

    pub(crate) fn fail(&self, error: &DriverError) {
        let message: Arc<str> = Arc::from(error.to_string());
        let mut registry = self.registry();
        registry.failure.get_or_insert(message);
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
        let first = std::thread::spawn(move || first_runtime.resolve_context::<TestContext>(&()).unwrap());
        let second = std::thread::spawn(move || runtime.resolve_context::<TestContext>(&()).unwrap());
        barrier.wait();

        let _ = first.join().unwrap();
        let _ = second.join().unwrap();

        assert_eq!(CONTEXT_CALLS.load(Ordering::SeqCst), 1);
    }
}
