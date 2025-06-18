// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};

use crate::pal::{
    CompletionQueueFacade, ElementaryOperationKey, MemoryPoolFacade, Platform, PlatformFacade,
};
use crate::{BoundPrimitiveRegistry, ERR_POISONED_LOCK, Operation, Runtime, SystemTaskCategory};

type ActiveOperationMap = HashMap<ElementaryOperationKey, Operation>;

/// Contains all resources of a single instance of the I/O driver which need to be shared between
/// the collaborating I/O driver and I/O context.
///
/// # Ownership
///
/// The resources are jointly owned by the I/O driver, the I/O context and related I/O types,
/// which share an instance of this type via `Arc`.
///
/// The type uses interior mutability to facilitate mutation of any resources it contains.
#[derive(Debug)]
pub struct Resources {
    /// Processing I/O completions requires write access, while binding I/O primitives and
    /// obtaining wakers requires read access and can happen concurrently.
    ///
    /// TODO: Not a hot path but we can probably relax this. The important part is that one could
    /// not perform I/O completion processing multiple times concurrently. However, there is no
    /// reason to block the other stuff while I/O processing happens. Well, at least on Windows.
    /// Revisit this when we incorporate Linux and have a better idea of platform-agnostic behavior.
    completion_queue: RwLock<CompletionQueueFacade>,

    memory_pool: MemoryPoolFacade,

    /// The currently active I/O operations, keyed by the elementary operation key, which will be
    /// present in the completion notifications of each I/O operation. Exclusive access required
    /// to add operations or to remove them when they complete.
    operations: Mutex<ActiveOperationMap>,

    /// Keeps track of I/O primitives that are bound to this I/O context. The purpose is to prevent
    /// resource leaks by preventing the I/O subsystem from being dropped until all bound I/O
    /// primitives are dropped.
    primitives: RwLock<BoundPrimitiveRegistry>,

    /// We use dynamic dispatch here to avoid the runtime type parameter polluting every I/O type.
    /// This is not on the hottest path, so should be fine from an efficiency point of view as the
    /// runtime is mostly involved in the creation/destruction flows and not during read/write.
    rt: Arc<Box<dyn Runtime>>,

    pal: PlatformFacade,
}

impl Resources {
    pub(crate) fn new(rt: Arc<Box<dyn Runtime>>, pal: PlatformFacade) -> Self {
        Self {
            completion_queue: pal.new_completion_queue().into(),
            memory_pool: pal.new_memory_pool(),
            operations: HashMap::new().into(),
            primitives: BoundPrimitiveRegistry::new().into(),
            rt,
            pal,
        }
    }

    pub(crate) fn completion_queue(&self) -> RwLockReadGuard<'_, CompletionQueueFacade> {
        self.completion_queue.read().expect(ERR_POISONED_LOCK)
    }

    pub(crate) fn completion_queue_mut(&self) -> RwLockWriteGuard<'_, CompletionQueueFacade> {
        self.completion_queue.write().expect(ERR_POISONED_LOCK)
    }

    pub(crate) const fn memory_pool(&self) -> &MemoryPoolFacade {
        &self.memory_pool
    }

    pub(crate) fn operations_mut(&self) -> MutexGuard<ActiveOperationMap> {
        self.operations.lock().expect(ERR_POISONED_LOCK)
    }

    pub(crate) fn primitives(&self) -> RwLockReadGuard<'_, BoundPrimitiveRegistry> {
        self.primitives.read().expect(ERR_POISONED_LOCK)
    }

    pub(crate) fn primitives_mut(&self) -> RwLockWriteGuard<'_, BoundPrimitiveRegistry> {
        self.primitives.write().expect(ERR_POISONED_LOCK)
    }

    pub(crate) fn runtime(&self) -> &Arc<Box<dyn Runtime>> {
        &self.rt
    }

    pub(crate) const fn pal(&self) -> &PlatformFacade {
        &self.pal
    }

    pub(crate) async fn execute_system_task<F, R>(&self, category: SystemTaskCategory, body: F) -> R
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        let (tx, rx) = oneshot::channel();

        self.enqueue_system_task(category, || {
            // We do not check for the error here because we do not care whether anyone is awaiting.
            let _ = tx.send(body());
        });

        rx.await
            .expect("system task failed before reporting result")
    }

    pub(crate) fn enqueue_system_task<F>(&self, category: SystemTaskCategory, body: F)
    where
        F: FnOnce() + Send + 'static,
    {
        self.rt.enqueue_system_task(category, Box::new(body));
    }
}