// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::{Any, TypeId};
use std::collections::hash_map::Entry;
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard};
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
    role: DriverRole,
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

#[derive(Default)]
struct Registry {
    contexts: HashMap<TypeId, Arc<RegisteredContext>>,
    failure: Option<Arc<str>>,
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
        if let Some(registered) = self.registered_context::<C>()? {
            return Ok(Resolution {
                context: registered.clone_context::<C>(),
                role: registered.role,
            });
        }

        let candidate = C::context(options.clone());
        let registered = {
            let mut registry = self.registry();
            Self::check_failure(&registry)?;
            let primary_available = registry.contexts.is_empty();
            match registry.contexts.entry(TypeId::of::<C>()) {
                Entry::Occupied(entry) => Arc::clone(entry.get()),
                Entry::Vacant(entry) => {
                    let registered = Arc::new(RegisteredContext {
                        context: Mutex::new(Box::new(NeutralContext(candidate))),
                        role: if primary_available {
                            DriverRole::Primary
                        } else {
                            DriverRole::Secondary
                        },
                    });
                    entry.insert(Arc::clone(&registered));
                    registered
                }
            }
        };
        Ok(Resolution {
            context: registered.clone_context::<C>(),
            role: registered.role,
        })
    }

    pub(crate) fn check_failed(&self) -> Result<(), DriverError> {
        Self::check_failure(&self.registry())
    }

    pub(crate) fn fail(&self, error: &DriverError) {
        let mut registry = self.registry();
        registry.failure.get_or_insert_with(|| Arc::from(error.to_string()));
    }

    pub(crate) const fn sink(&self) -> &Sink {
        &self.sink
    }

    fn registered_context<C>(&self) -> Result<Option<Arc<RegisteredContext>>, DriverError>
    where
        C: IoContext,
    {
        let registry = self.registry();
        Self::check_failure(&registry)?;
        Ok(registry.contexts.get(&TypeId::of::<C>()).cloned())
    }

    fn check_failure(registry: &Registry) -> Result<(), DriverError> {
        match &registry.failure {
            Some(message) => Err(DriverError::from_message(message.as_ref())),
            None => Ok(()),
        }
    }

    fn registry(&self) -> MutexGuard<'_, Registry> {
        self.registry
            .lock()
            .expect("a poisoned I/O context registry indicates a runtime invariant failure")
    }
}
