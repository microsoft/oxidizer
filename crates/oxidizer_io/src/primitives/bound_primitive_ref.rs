// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::{Arc, Mutex};

use async_observable::Observable;
use derive_more::Display;
use tracing::{Level, event};

use crate::pal::{Primitive, PrimitiveFacade};
use crate::{
    AsNativePrimitivePrivate, ERR_POISONED_LOCK, PrimitiveRegistrationGuard, Resources,
    SystemTaskCategory,
};

/// A narrowed view over a `BoundPrimitive` that only exposes the API surface required for
/// I/O operations but does not expose resource management operations.
///
/// This is used primarily in the callbacks that execute the core logic of low-level I/O operations,
/// to grant limited access to the primitive so the callback can communicate with the platform.
#[derive(derive_more::Debug, Display)]
#[display("{pal_primitive}")]
pub struct BoundPrimitiveRef {
    // The  documentation above is true... for the purposes of the public API. Internally, however,
    // this is the "real" BoundPrimitive and the public BoundPrimitive is just a thin wrapper around
    // this, exposing public functions that this one keeps private.
    //
    // The purpose of this two-layer design is to give strong-typed borrow-checked certainty to
    // the I/O operation callback that it has a valid bound primitive, without allowing that
    // simple callback (whose only purpose is to start the I/O operation) to perform more complex
    // calls or even start new I/O operations, which could lead to all sorts of logical issues.
    // We need this two-layer because our I/O operation design allows the I/O operations to be
    // deferred for batching purposes, so we cannot rely on real-time borrow checking and need
    // to go via `Arc` and `Weak`.
    //
    // TODO: Once we have Linux + optimizations, review this design to see if we can cut out the
    // `Arc` and `Weak` logic. Currently this is here to give us design flexibility but if we can
    // make due with real-time borrow checking, we may want to do so to avoid the dynamic
    // lifetime tracking overhead.
    //
    // We start resource cleanup when BoundPrimitiveRef is dropped.
    //
    // This requires that both of these conditions are true:
    // * The BoundPrimitive is dropped or closed.
    // * The begin() callback of any associated I/O operation completes.
    //
    // Note that enqueued I/O operations (whose callbacks have not yet started) do not extend the
    // lifetime of the primitive - those operations are canceled when the primitive is dropped and
    // only hold weak references meanwhile.
    //
    // Private callers can observe resource release progress via observe_close(), which notifies
    // when everything has been cleaned up. Obviously, you need to observe_close() before the
    // instance is dropped!
    pal_primitive: PrimitiveFacade,

    // Set to None when cleanup starts.
    registration_guard: Option<PrimitiveRegistrationGuard>,

    // This is a poor simulation of a ManualResetEvent, which does not yet exist in
    // Rust in an async-compatible form. If we keep it after optimizations pass, make
    // a proper async Rust implementation of ManualResetEvent and use that instead.
    //
    // We use the mutex only for interior-mutable cloning, observers do not care about it.
    // Set to None when cleanup starts.
    closed_flag: Mutex<Option<Observable<bool>>>,

    #[debug(ignore)]
    resources: Arc<Resources>,
}

impl BoundPrimitiveRef {
    pub(crate) fn new(
        pal_primitive: PrimitiveFacade,
        registration_guard: PrimitiveRegistrationGuard,
        resources: Arc<Resources>,
    ) -> Self {
        Self {
            pal_primitive,
            registration_guard: Some(registration_guard),
            resources,
            closed_flag: Mutex::new(Some(Observable::new(false))),
        }
    }

    ///
    /// # Panics
    ///
    /// TODO: Document panics
    pub fn observe_close(&self) -> impl Future<Output = ()> + use<> {
        // The Mutex only guards the clone() call, observers are not synchronized.
        let mut closed_flag = {
            let closed_flag = self.closed_flag.lock().expect(ERR_POISONED_LOCK);

            closed_flag
                .as_ref()
                .expect("only taken on drop, so must be set")
                .clone()
        };

        async move {
            if closed_flag.synchronize() {
                return;
            }

            while !closed_flag.next().await {}
        }
    }
}

impl Drop for BoundPrimitiveRef {
    fn drop(&mut self) {
        // We need to leave `self` in a valid state, so we have to clone.
        let pal_primitive = self.pal_primitive.clone();

        event!(Level::TRACE, message = "drop()", primitive = %pal_primitive);

        let registration_guard = self
            .registration_guard
            .take()
            .expect("only taken on drop, so must be set");

        let mut closed_flag = self
            .closed_flag
            .lock()
            .expect(ERR_POISONED_LOCK)
            .take()
            .expect("only taken on drop, so must be set");

        self.resources.runtime().enqueue_system_task(
            SystemTaskCategory::ReleaseResources,
            Box::new(move || {
                event!(Level::TRACE, message = "closing native primitive", primitive = %pal_primitive);

                pal_primitive.close();

                // Notifies the I/O driver that we have completed cleanup. The I/O driver waits for
                // all primitives to finish their cleanup logic before exiting, to avoid resource leaks.
                drop(registration_guard);

                // Notifies any observers who are awaiting on this primitive to be closed.
                closed_flag.publish(true);
            }),
        );
    }
}

impl AsNativePrimitivePrivate for BoundPrimitiveRef {
    fn as_pal_primitive(&self) -> &crate::pal::PrimitiveFacade {
        &self.pal_primitive
    }
}