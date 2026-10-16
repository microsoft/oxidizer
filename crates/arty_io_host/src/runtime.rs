// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::{Any, TypeId};
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::Waker;

use arty_io_core::{DriverRole, IoContext};
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
    context: Box<dyn ErasedContext>,
    role: DriverRole,
}

#[derive(Default)]
struct Registry {
    contexts: HashMap<TypeId, RegisteredContext>,
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
    /// Panics if this runtime clone has not been relocated onto its worker.
    #[must_use]
    pub fn driver_host(&self, owner_waker: Waker) -> DriverHost {
        let thread = self
            .thread
            .clone()
            .expect("IoRuntime must be relocated onto its worker before creating a driver host");
        DriverHost::new(self.clone(), thread, owner_waker, self.sink.clone())
    }

    pub(crate) fn get_context<C>(&self, options: &C::ContextOptions) -> Resolution<C>
    where
        C: IoContext,
        C::ContextOptions: Clone,
    {
        let context_type = TypeId::of::<C>();
        let mut registry = self.registry();
        if let Some(registered) = registry.contexts.get(&context_type) {
            let context = registered
                .context
                .clone_context()
                .downcast::<C>()
                .expect("I/O contexts are stored under their own TypeId");
            return Resolution {
                context: *context,
                role: registered.role,
            };
        }

        let role = if registry.contexts.is_empty() {
            DriverRole::Primary
        } else {
            DriverRole::Secondary
        };
        let context = C::context(options.clone());
        registry.contexts.insert(
            context_type,
            RegisteredContext {
                context: Box::new(NeutralContext(context)),
                role,
            },
        );
        let registered = registry
            .contexts
            .get(&context_type)
            .expect("the context was inserted immediately above");
        let context = registered
            .context
            .clone_context()
            .downcast::<C>()
            .expect("I/O contexts are stored under their own TypeId");
        Resolution { context: *context, role }
    }

    fn registry(&self) -> MutexGuard<'_, Registry> {
        self.registry
            .lock()
            .expect("a poisoned I/O context registry indicates a runtime invariant failure")
    }
}
