// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::{Any, TypeId, type_name};
use std::collections::HashMap;
use std::fmt;
use std::task::Waker;

use thread_aware_core::Thread;

use crate::{CompletionDomain, CompletionService, DriverError, SystemTasks};

/// Placement and runtime facilities supplied when one driver instance is created.
///
/// The runtime assembles this on the final owning thread. Native client services are scoped to
/// the selected waiter's domain and may themselves be thread-local. This context is therefore
/// neither `Send` nor `Sync`; mobile consumers receive a separate [`IoContext`](crate::IoContext).
pub struct DriverContext {
    thread: Thread,
    system_tasks: SystemTasks,
    domain: CompletionDomain,
    readiness_waker: Waker,
    completion_services: HashMap<TypeId, Box<dyn Any>>,
}

impl DriverContext {
    /// Creates the context for one driver instance.
    ///
    /// This constructor is intended for runtime implementations and driver tests.
    #[must_use]
    pub fn new(thread: Thread, system_tasks: SystemTasks, domain: CompletionDomain, readiness_waker: Waker) -> Self {
        Self {
            thread,
            system_tasks,
            domain,
            readiness_waker,
            completion_services: HashMap::new(),
        }
    }

    /// Returns the async worker this driver instance serves.
    #[must_use]
    pub const fn thread(&self) -> &Thread {
        &self.thread
    }

    /// Returns the runtime-owned facility for blocking I/O system work.
    #[must_use]
    pub const fn system_tasks(&self) -> &SystemTasks {
        &self.system_tasks
    }

    /// Returns the collection domain whose waiter the runtime will drive.
    #[must_use]
    pub const fn completion_domain(&self) -> &CompletionDomain {
        &self.domain
    }

    /// Returns the runtime-provided notification handle for this driver's service.
    ///
    /// Native adapters signal this after publishing records or readiness that require driver
    /// service. The runtime latches the source's readiness before interrupting its domain waiter.
    /// This handle is not a transport for operation records and must not run driver service inline.
    /// It remains memory-safe after registration rollback, driver shutdown, or driver destruction;
    /// late notifications must not be redirected to a reused source identity.
    #[must_use]
    pub const fn readiness_waker(&self) -> &Waker {
        &self.readiness_waker
    }

    /// Supplies one typed client capability from the selected completion domain.
    ///
    /// This is a construction-time operation; completion records do not use this map.
    ///
    /// # Errors
    ///
    /// Returns a wrong-domain error for a foreign service, or a duplicate-service error if
    /// another value with the same client type has already been supplied.
    pub fn with_completion_service<T: 'static>(mut self, service: CompletionService<T>) -> Result<Self, DriverError> {
        if !self.domain.is_same(service.domain()) {
            return Err(DriverError::wrong_domain(type_name::<T>()));
        }
        if self.completion_services.contains_key(&TypeId::of::<T>()) {
            return Err(DriverError::duplicate_service(type_name::<T>()));
        }
        self.completion_services.insert(TypeId::of::<T>(), Box::new(service.value));
        Ok(self)
    }

    /// Borrows a native client capability of type `T`.
    ///
    /// Clone an appropriate client handle if the driver needs to retain it after creation.
    /// Native adapter packages define the client interfaces and their registration/retirement
    /// rules; the core does not interpret their operation or buffer types.
    ///
    /// # Errors
    ///
    /// Returns an unsupported-configuration error when this domain does not supply `T`.
    #[expect(
        clippy::missing_panics_doc,
        reason = "the typed insertion API establishes the internal type-id invariant"
    )]
    pub fn completion_service<T: 'static>(&self) -> Result<&T, DriverError> {
        let service = self
            .completion_services
            .get(&TypeId::of::<T>())
            .ok_or_else(|| DriverError::missing_service(type_name::<T>()))?;
        Ok(service
            .downcast_ref::<T>()
            .expect("completion service type ids are recorded together with their values"))
    }

    pub(crate) fn has_completion_service(&self, id: TypeId) -> bool {
        self.completion_services.contains_key(&id)
    }
}

impl fmt::Debug for DriverContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("thread", &self.thread)
            .field("domain", &self.domain)
            .field("completion_service_count", &self.completion_services.len())
            .finish_non_exhaustive()
    }
}
