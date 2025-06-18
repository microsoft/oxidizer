// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::Arc;

use derive_more::Display;

use crate::mem::{Sequence, SequenceBuilder};
use crate::{
    AsNativePrimitivePrivate, BoundPrimitiveRef, ControlOperation, ReadOperation, Resources,
    WriteOperation,
};

/// An I/O primitive that has been bound to the I/O subsystem and can serve as the target of
/// low-level I/O operations.
///
/// The I/O subsystem receives completion notifications for this I/O primitive and is responsible
/// for automatic lifetime management, releasing platform resources and potentially canceling
/// outstanding operations associated with the I/O primitive when the instance is dropped.
///
/// You can obtain instances of this from [`Context::bind_primitive()`][1].
///
/// # Resource management
///
/// Resources associated with the primitive are automatically released shortly after the instance
/// and any shared references to it are dropped. This cleanup is an asynchronous operation and may
/// not complete immediately.
///
/// If you want to wait for the resources to be released, you can explicitly call
/// [`close()`][Self::close] and await the returned future.
///
#[doc = include_str!("../../doc/snippets/primitive_close_hard.md")]
///
/// # Thread safety
///
/// This type is thread-mobile. If you need to share access to multiple threads, you can use
/// an `Arc<RwLock<BoundPrimitive>>` or `Arc<RwLock<Option<BoundPrimitive>>>`, which supports
/// consuming the instance (necessary for calling `close()`)
///
/// Starting I/O operations only requires a shared reference to the primitive and is safe to do
/// from any thread. Only `close()` requires an exclusive reference and consumes the primitive.
///
/// To ensure proper lifetime management, all methods that can be used to extract a platform-
/// specific native resource handle only return single-threaded types (either types that are
/// naturally single-threaded or in a `SingleThreaded` wrapper). Only the `BoundPrimitive` may
/// be moved across threads - any native forms of the primitive must be separately requested on
/// every thread from this type.
///
/// [1]: crate::Context::bind_primitive
#[derive(derive_more::Debug, Display)]
#[display("{inner}")]
pub struct BoundPrimitive {
    inner: Arc<BoundPrimitiveRef>,

    #[debug(ignore)]
    resources: Arc<Resources>,
}

impl BoundPrimitive {
    pub(crate) fn new(inner: BoundPrimitiveRef, resources: Arc<Resources>) -> Self {
        Self {
            inner: Arc::new(inner),
            resources,
        }
    }

    /// Prepares to execute an I/O operation that sends a control message to an I/O primitive,
    /// executing some custom logic that is not categorizable under the other operation types.
    ///
    /// This type of operation does not read or write any data through memory owned by the
    /// I/O subsystem - any input or output structures must be owned by the caller.
    #[must_use]
    pub fn control(&self) -> ControlOperation {
        ControlOperation::new(Arc::downgrade(&self.inner), Arc::clone(&self.resources))
    }

    /// Prepares to execute an I/O operation that reads some bytes of data.
    ///
    /// The underlying I/O primitive may return any number of bytes, from zero to filling
    /// the entire [`SequenceBuilder`]. A result of zero bytes indicates end of the data stream.
    ///
    /// The caller must provide the memory that will hold the read bytes, by supplying a
    /// [`SequenceBuilder`] obtained from the same I/O context. The provided sequence builder
    /// must have a nonzero capacity.
    #[must_use]
    pub fn read_bytes(&self, buffer: SequenceBuilder) -> ReadOperation {
        ReadOperation::new(
            Arc::downgrade(&self.inner),
            buffer,
            Arc::clone(&self.resources),
        )
    }

    /// Prepares to execute an I/O operation that writes some bytes of data.
    ///
    /// A successful write operation will write all the bytes in the provided sequence.
    ///
    /// The caller must provide the bytes of data to write as a [`Sequence`]. The provided
    /// sequence must not be empty (as that likely indicates a programming error by the caller).
    ///
    /// The logical operation is split into one or more elementary operations that each operate on
    /// up to `MAX_CHUNKS` consecutive chunks of bytes. Specify `MAX_CHUNKS=1` to start a new
    /// elementary operation for each chunk of consecutive bytes. This may be required if the
    /// underlying native I/O API is not capable of performing vectored I/O.
    #[must_use]
    pub fn write_bytes<const MAX_CHUNKS: usize>(
        &self,
        data: Sequence,
    ) -> WriteOperation<MAX_CHUNKS> {
        WriteOperation::new(
            Arc::downgrade(&self.inner),
            data,
            Arc::clone(&self.resources),
        )
    }

    /// Releases the resources associated with the primitive, abandoning any pending I/O operations.
    ///
    /// Calling this is optional and only needed if you want to wait for the resource release to
    /// be completed. When the primitive is dropped, the I/O subsystem will automatically release
    /// the resources associated with it (potentially with some delay - cleanup is asynchronous).
    ///
    #[doc = include_str!("../../doc/snippets/primitive_close_hard.md")]
    pub async fn close(self) {
        let close_observer = self.inner.observe_close();

        // This will drop the reference to the inner `BoundPrimitiveRef` and eventually start
        // the cleanup process. It may still be alive for a bit if something else is holding
        // a reference (e.g. an ongoing I/O operation).
        drop(self);

        close_observer.await;
    }

    /// For testing purposes only, allows the inner `BoundPrimitiveRef` to be accessed directly.
    #[cfg(test)]
    pub(crate) const fn inner(&self) -> &Arc<BoundPrimitiveRef> {
        &self.inner
    }
}

impl AsNativePrimitivePrivate for BoundPrimitive {
    fn as_pal_primitive(&self) -> &crate::pal::PrimitiveFacade {
        self.inner.as_pal_primitive()
    }
}

#[cfg(test)]
mod tests {
    use std::pin::pin;
    use std::task;

    use futures::task::noop_waker_ref;

    use super::*;
    use crate::pal::{MockPlatform, MockPrimitive, PlatformFacade};
    use crate::testing::{
        expect_elementary_operations, use_default_memory_pool, use_simulated_completion_queue,
        with_partial_io_test_harness_and_platform,
    };

    #[test]
    fn pal_closed_on_drop() {
        // Even though this test does not care about the PAL, we need to use
        // a mock platform because Miri cannot work under a real platform.
        let mut platform = MockPlatform::new();

        use_default_memory_pool::<1234>(&mut platform);
        expect_elementary_operations(0, &mut platform);

        use_simulated_completion_queue(&mut platform);

        let platform = PlatformFacade::from_mock(platform);

        with_partial_io_test_harness_and_platform(platform, async move |test_harness| {
            let resources = test_harness.context.resources();

            let mut pal_primitive = MockPrimitive::new();

            // We expect the primitive to be cloned once, and the clone closed.
            // This is just how the implementation works today (adjust if needed).
            pal_primitive.expect_clone().times(1).returning(|| {
                let mut clone = MockPrimitive::new();
                clone.expect_close().times(1).return_const(());
                clone
            });

            let registration_guard = { resources.primitives().register() };

            let primitive_ref = BoundPrimitiveRef::new(
                pal_primitive.into(),
                registration_guard,
                Arc::clone(resources),
            );

            let primitive = BoundPrimitive::new(primitive_ref, Arc::clone(resources));

            // We expect this to immediately schedule a system task to close the PAL primitive
            // because there are no shared references to the primitive which would delay this.
            drop(primitive);

            // The I/O driver will wait for the primitive to be closed before exiting.
        });
    }

    #[test]
    fn pal_closed_on_explicit_close() {
        // Even though this test does not care about the PAL, we need to use
        // a mock platform because Miri cannot work under a real platform.
        let mut platform = MockPlatform::new();

        use_default_memory_pool::<1234>(&mut platform);
        expect_elementary_operations(0, &mut platform);

        use_simulated_completion_queue(&mut platform);

        let platform = PlatformFacade::from_mock(platform);

        with_partial_io_test_harness_and_platform(platform, async move |test_harness| {
            let resources = test_harness.context.resources();

            let mut pal_primitive = MockPrimitive::new();

            // We expect the primitive to be cloned once, and the clone closed.
            // This is just how the implementation works today (adjust if needed).
            pal_primitive.expect_clone().times(1).returning(|| {
                let mut clone = MockPrimitive::new();
                clone.expect_close().times(1).return_const(());
                clone
            });

            let registration_guard = {
                let registry = resources.primitives_mut();
                registry.register()
            };

            let primitive_ref = BoundPrimitiveRef::new(
                pal_primitive.into(),
                registration_guard,
                Arc::clone(resources),
            );

            let primitive = BoundPrimitive::new(primitive_ref, Arc::clone(resources));

            // We expect this to only complete once the primitive has been closed.
            primitive.close().await;

            assert!(resources.primitives().is_empty());
        });
    }

    #[test]
    fn close_completes_only_once_primitive_really_closed() {
        // We expect .close().await to suspend until the PAL notifies that
        // the primitive has really been closed.

        // Even though this test does not care about the PAL, we need to use
        // a mock platform because Miri cannot work under a real platform.
        let mut platform = MockPlatform::new();

        use_default_memory_pool::<1234>(&mut platform);
        expect_elementary_operations(0, &mut platform);

        use_simulated_completion_queue(&mut platform);

        let platform = PlatformFacade::from_mock(platform);

        with_partial_io_test_harness_and_platform(platform, async move |test_harness| {
            let resources = test_harness.context.resources();

            let mut pal_primitive = MockPrimitive::new();

            // We expect the primitive to be cloned once, and the clone closed.
            // This is just how the implementation works today (adjust if needed).
            pal_primitive.expect_clone().times(1).returning(|| {
                let mut clone = MockPrimitive::new();
                clone.expect_close().times(1).return_const(());
                clone
            });

            let registration_guard = {
                let registry = resources.primitives_mut();
                registry.register()
            };

            let primitive_ref = BoundPrimitiveRef::new(
                pal_primitive.into(),
                registration_guard,
                Arc::clone(resources),
            );

            let primitive = BoundPrimitive::new(primitive_ref, Arc::clone(resources));

            // This will keep the inner primitive alive, just as if there were an ongoing I/O
            // operation keeping it alive. While this is alive, `close()` will not complete.
            let inner = Arc::clone(primitive.inner());

            // We expect this to only complete once the primitive has been closed.
            let mut close_future = pin!(primitive.close());
            assert!(
                close_future
                    .as_mut()
                    .poll(&mut task::Context::from_waker(noop_waker_ref()))
                    .is_pending()
            );

            drop(inner);

            // Now we expect the close future to complete. We will directly await this because
            // it is not strictly guaranteed to complete on the first poll (there is an observer
            // black box in the call chain, who knows what it does or how it works!).
            close_future.await;
        });
    }
}