// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::Debug;
use std::pin::Pin;

use crate::UserResource;
use crate::mem::MemoryGuard;
use crate::pal::{
    ElementaryOperation, ElementaryOperationFacade, ElementaryOperationKey, Platform,
    PlatformFacade,
};

/// Metadata for a single elementary asynchronous I/O operation issued to the operating system.
pub struct Operation {
    // Extends the lifetime of the I/O blocks referenced by the operation, representing the
    // platform's ownership of this memory in the Rust universe.
    memory_guard: MemoryGuard,

    /// The part of the operation metadata that is visible to the operating system.
    /// This is pinned because we hand out pointers to this to the operating system.
    elementary: Pin<Box<ElementaryOperationFacade>>,

    // Reports the number of bytes read/written by the operation.
    result_tx: Option<oneshot::Sender<crate::Result<u32>>>,
    result_rx: Option<oneshot::Receiver<crate::Result<u32>>>,

    // Arbitrary droppable resources the code that issued the operation wants to keep alive until
    // the operation completes. The I/O blocks are automatically taken care of but some I/O
    // operations may involve additional custom resources, represented here in abstract form.
    user_resources: Option<Box<dyn UserResource>>,
}

impl Operation {
    pub(crate) fn new(
        offset: u64,
        memory_guard: MemoryGuard,
        user_resources: Option<Box<dyn UserResource>>,
        pal: &PlatformFacade,
    ) -> Self {
        let (result_tx, result_rx) = oneshot::channel();

        let elementary = Box::pin(pal.new_elementary_operation(offset));

        Self {
            elementary,
            memory_guard,
            result_tx: Some(result_tx),
            result_rx: Some(result_rx),
            user_resources,
        }
    }

    pub(crate) fn elementary_operation_key(&self) -> ElementaryOperationKey {
        self.elementary.as_ref().key()
    }

    pub(crate) const fn take_result_rx(&mut self) -> oneshot::Receiver<crate::Result<u32>> {
        self.result_rx
            .take()
            .expect("operation result receiver already taken")
    }

    pub(crate) const fn take_result_tx(&mut self) -> oneshot::Sender<crate::Result<u32>> {
        self.result_tx
            .take()
            .expect("operation result sender already taken")
    }
}

impl Debug for Operation {
    #[cfg_attr(test, mutants::skip)] // There is no API contract this needs to satisfy.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Operation")
            .field("elementary", &self.elementary)
            .field("memory_guard", &self.memory_guard)
            .field("result_tx", &self.result_tx)
            .field("result_rx", &self.result_rx)
            .field("user_resources", &self.user_resources)
            .finish()
    }
}