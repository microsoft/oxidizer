// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::{Any, TypeId, type_name};
use std::collections::HashMap;
use std::fmt;
use std::task::Waker;

use thread_aware_core::Thread;

use crate::{DriverError, SystemTasks};

/// Placement and runtime facilities supplied when one driver instance is created.
///
/// The runtime assembles this on the final owning thread, then asks that worker's collector to
/// attach its own client capabilities through
/// [`CompletionWaiter::attach_clients`](crate::CompletionWaiter::attach_clients). Native clients
/// may themselves be thread-local, so this context is neither [`Send`] nor [`Sync`]; mobile
/// consumers receive a separate [`IoContext`](crate::IoContext).
///
/// A client type is matched by its exact type identity. Sharing this core crate is not enough for
/// a driver and a native adapter to interoperate: they must agree on the same adapter crate, the
/// same client interface, and the same compiled version of it. Two builds of one adapter whose
/// types are distinct simply do not match, and the lookup reports an unsupported configuration.
pub struct DriverContext {
    thread: Thread,
    system_tasks: SystemTasks,
    readiness_waker: Waker,
    completion_services: HashMap<TypeId, Box<dyn Any>>,
}

impl DriverContext {
    /// Creates the context for one driver instance.
    ///
    /// This constructor is intended for runtime implementations and driver tests.
    #[must_use]
    pub fn new(thread: Thread, system_tasks: SystemTasks, readiness_waker: Waker) -> Self {
        Self {
            thread,
            system_tasks,
            readiness_waker,
            completion_services: HashMap::new(),
        }
    }

    /// Returns the async worker this driver instance serves.
    ///
    /// These coordinates include [`Thread::numa_node`] for locality-aware resource allocation.
    #[must_use]
    pub const fn thread(&self) -> &Thread {
        &self.thread
    }

    /// Returns the runtime-owned facility for blocking I/O system work.
    #[must_use]
    pub const fn system_tasks(&self) -> &SystemTasks {
        &self.system_tasks
    }

    /// Returns the runtime-provided notification handle for this driver's service.
    ///
    /// Native adapters signal this after publishing records or readiness that require driver
    /// service. The runtime latches the source's readiness before interrupting its collector.
    /// This handle is not a transport for operation records and must not run driver service
    /// inline. It remains memory-safe after registration rollback, driver shutdown, or driver
    /// destruction; late notifications must not be redirected to a reused source identity.
    #[must_use]
    pub const fn readiness_waker(&self) -> &Waker {
        &self.readiness_waker
    }

    /// Supplies one typed native client capability to the driver being created.
    ///
    /// Collectors normally attach their own clients through
    /// [`CompletionWaiter::attach_clients`](crate::CompletionWaiter::attach_clients); a runtime
    /// with no native collector can also use this directly. The core cannot establish that a
    /// supplied client is backed by the collector that will service this worker, so a runtime
    /// must assemble each context from one coherent native arrangement.
    ///
    /// This is a construction-time operation; completion records do not use this map. Rejection
    /// reflects only the type's current occupancy: once a value of type `T` is taken through
    /// [`take_completion_service`](Self::take_completion_service), a later value of that same
    /// type may be supplied again.
    ///
    /// # Errors
    ///
    /// Returns a duplicate-client error if a value of this client type is currently supplied.
    /// A client is never silently replaced.
    pub fn with_completion_service<T: 'static>(mut self, value: T) -> Result<Self, DriverError> {
        if self.completion_services.contains_key(&TypeId::of::<T>()) {
            return Err(DriverError::duplicate_service(type_name::<T>()));
        }
        drop(self.completion_services.insert(TypeId::of::<T>(), Box::new(value)));
        Ok(self)
    }

    /// Takes ownership of a native client capability of type `T`.
    ///
    /// This removes the value from the context: a repeated take of the same type reports the
    /// missing-client error, and the type may be supplied again through
    /// [`with_completion_service`](Self::with_completion_service). Extraction itself performs no
    /// native operation. Take every required client before starting native registration, so a
    /// missing requirement can abort without a partial driver registration.
    /// Native adapter packages define the client interfaces and their registration and retirement
    /// rules; the core does not interpret their operation or buffer types.
    ///
    /// # Errors
    ///
    /// Returns an unsupported-configuration error when this worker does not currently supply `T`.
    #[expect(
        clippy::missing_panics_doc,
        reason = "the typed insertion API establishes the internal type-id invariant"
    )]
    pub fn take_completion_service<T: 'static>(&mut self) -> Result<T, DriverError> {
        let service = self
            .completion_services
            .remove(&TypeId::of::<T>())
            .ok_or_else(|| DriverError::missing_service(type_name::<T>()))?;
        Ok(*service
            .downcast::<T>()
            .expect("completion service type ids are recorded together with their values"))
    }
}

impl fmt::Debug for DriverContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("thread", &self.thread)
            .field("completion_service_count", &self.completion_services.len())
            .finish_non_exhaustive()
    }
}
