// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::{Any, TypeId};
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard};

use arty_io_core::{DriverRole, IoContext};
use foldhash::HashMap;

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

impl Default for IoRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl IoRuntime {
    /// Creates an empty runtime-wide I/O context registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            registry: Arc::new(Mutex::new(Registry::default())),
        }
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
