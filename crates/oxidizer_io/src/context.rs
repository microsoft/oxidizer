// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::mem::SequenceBuilder;
use crate::pal::{CompletionQueue, MemoryPool};
use crate::{
    BoundPrimitive, BoundPrimitiveRef, ReserveOptions, Resources, SystemTaskCategory,
    UnboundPrimitive, nz,
};
use oxidizer_mem::ProvideMemory;
use std::num::NonZero;
use std::sync::Arc;

/// The I/O context provides low-level I/O capabilities to implementations of I/O endpoints
/// such as `File`, `Socket`, `TcpServer` and others.
///
/// In most cases, service code does not need to interact with the I/O context,
/// merely passing the context to I/O endpoints when they are created.
///
/// # Obtaining an I/O context
///
/// It is the responsibility of the async task runtime to provide the I/O context to callers
/// interested in performing I/O. The exact mechanism for obtaining a reference depends on the
/// implementation details of the async task runtime.
///
/// I/O endpoints like `File` will typically take an `Arc<Context>` in their constructor and
/// thereafter interact with the I/O context in the following ways:
///
/// * The I/O context can reserve memory (i.e. provide a [`SequenceBuilder`][4]) when requested to
///   by the I/O endpoint. The I/O endpoint may specify fine-tuning options to help optimize the
///   memory for a particular I/O endpoint and configuration/operation (e.g. via contiguous
///   memory block length preferences or memory alignment requirements).
/// * The I/O context can bind I/O primitives (e.g. file handles) to the I/O subsystem, which allows
///   I/O operations to be performed on the I/O primitive. This is done by calling
///   [`Context::bind_primitive()`][13].
///
/// # I/O primitives
///
/// An **I/O primitive** is an operating system I/O concept like "file handle", "socket" or "pipe".
/// The Oxidizer I/O subsystem works with I/O primitives.
///
/// On top of I/O primitives, libraries (either part of Oxidizer SDK or not) can build
/// **I/O endpoints**, which are Rust types like `TcpConnection`, `Socket` or `File` with convenient
/// APIs. This crate does not provide any I/O endpoints, only low-level APIs for working with
/// I/O primitives.
///
/// To use an I/O primitive, [it must be bound to an I/O context][13]. This is typically done
/// by the I/O endpoint implementation when the endpoint is created.
///
/// The following types of operations are supported on a bound I/O primitive:
///
/// * [Read bytes][8] - the operating system obtains some bytes and delivers them to the I/O
///   endpoint (e.g. reading from a file).
/// * [Write bytes][9] - the I/O endpoint obtains some bytes from the caller and delivers them to
///   the operating system (e.g. writing to a file).
/// * [Control][10] - the I/O endpoint requests the operating system to perform some
///   control/configuration operation that neither reads nor writes bytes (e.g. connecting
///   a socket to a remote endpoint).
///
/// Starting an operation creates the Rust future that can be awaited to observe the results of
/// the operation.
///
/// After the I/O primitive is bound, executing an I/O operation consists of approximately the
/// following steps:
///
/// 1. Prepare any data (as a [`Sequence`][3]) or memory capacity (as a [`SequenceBuilder`][4])
///    if the operation reads or writes data.
/// 1. Register a new operation on the bound primitive (e.g. by calling
///    [`BoundPrimitive::read_bytes()`][12]), providing
///    the data or memory from the previous step. The I/O subsystem takes ownership of the data
///    or memory at this point (though recall that a [`Sequence`][3] can be cloned at zero cost).
/// 1. Optionally, specify extended operation configuration on the returned value from the previous
///    step (e.g. `.with_offset()` if operating on a seekable I/O primitive).
/// 1. Start the operation by calling [`.begin()`][15]. This takes a callback as input and returns
///    a future that can be awaited to observe the result of the operation.
///
/// The callback provided to `begin()` receives a view over the primitive and an arguments object,
/// the latter of which provides limited access to the data or memory associated with the operation
/// and mechanisms to report status. The purpose of the callback is to start the native
/// asynchronous I/O operation by performing a call into the operating system, instructing it to
/// operate on the native operating system I/O primitive in some way.
///
/// ```ignore
/// // Partial extract from `tests/file_write_windows.rs`.
/// // The call into the operating system occurs at `WriteFile()`.
/// file
///     .write_bytes::<1>(bytes) // 1 means non-vectored write.
///     .with_offset(offset)
///     .begin(move |primitive, mut args| {
///         // SAFETY: We are not allowed to reuse this for multiple calls and we are only
///         // allowed to use it with the primitive given to this callback. We obey the rules.
///         let overlapped = unsafe { args.overlapped() };
///
///         let chunk = args
///             .chunks()
///             .next()
///             .expect("a write with 0 chunks is not a legal operation - there must be one");
///
///         // SAFETY: The buffer must remain valid for the duration of any asynchronous
///         // I/O, which is guaranteed by the I/O subsystem that calls us.
///         let result = unsafe {
///             WriteFile(
///                 *primitive.as_handle(),
///                 Some(chunk),
///                 Some(args.bytes_written_synchronously_as_mut()),
///                 Some(overlapped),
///             )
///         };
///
///         BeginResult::from_windows_result(result)
///     })
///     .await
/// ```
///
/// It may be that an I/O operation completes synchronously, in which case the callback will return
/// the immediate result. If the I/O operation is enqueued for asynchronous operation, the callback
/// simply returns a flag to indicate this (in which case the I/O driver becomes responsible for
/// observing I/O completion). This is signaled via [`BeginResult`][21], returned from the callback.
///
/// More detailed example code can be seen in the integration tests `file_write_windows.rs` and
/// similar, which may be considered reference implementations.
///
/// [1]: crate::Context
/// [3]: crate::mem::Sequence
/// [4]: crate::mem::SequenceBuilder
/// [5]: crate::Driver
/// [8]: crate::ReadOperation
/// [9]: crate::WriteOperation
/// [10]: crate::ControlOperation
/// [12]: crate::BoundPrimitive::read_bytes
/// [13]: crate::Context::bind_primitive
/// [15]: crate::ReadOperation::begin
/// [16]: crate::mem::Sequence::into_bytes
/// [17]: https://docs.rs/bytes/latest/bytes/buf/trait.Buf.html
/// [18]: https://docs.rs/bytes/latest/bytes/struct.Bytes.html
/// [19]: crate::mem
/// [20]: https://docs.rs/bytes/latest/bytes/buf/trait.BufMut.html
/// [21]: crate::BeginResult
#[derive(Debug, Clone)]
pub struct Context {
    resources: Arc<Resources>,
}

impl Context {
    pub(crate) const fn new(resources: Arc<Resources>) -> Self {
        Self { resources }
    }

    /// Binds an I/O primitive to the I/O context. This enables the I/O operations associated with
    /// this primitive to be processed by the I/O driver associated with the I/O context.
    ///
    /// This is intended to be used by I/O endpoints like `File` when creating the I/O primitives
    /// that they will use to interact with the operating system's I/O capabilities.
    ///
    /// Returns the bound I/O primitive. Dropping the instance returned will release all resources
    /// associated with the primitive, including platform resources, and may cancel any pending
    /// I/O operations on it.
    ///
    /// The I/O primitive must be configured to support the platform's native asynchronous I/O
    /// mechanism (e.g. `FILE_FLAG_OVERLAPPED` must be specified at primitive creation on Windows).
    /// This refers to I/O completion ports on Windows, and `io_uring` on Linux.
    pub fn bind_primitive(
        &self,
        into_primitive: impl Into<UnboundPrimitive>,
    ) -> crate::Result<BoundPrimitive> {
        let primitive: UnboundPrimitive = into_primitive.into();

        self.resources
            .completion_queue()
            .bind(&primitive.pal_primitive)?;

        let registration_guard = self.resources.primitives_mut().register();

        let primitive_ref = BoundPrimitiveRef::new(
            primitive.pal_primitive,
            registration_guard,
            Arc::clone(&self.resources),
        );

        Ok(BoundPrimitive::new(
            primitive_ref,
            Arc::clone(&self.resources),
        ))
    }

    /// Reserves at least `len` bytes of memory from the I/O subsystem,
    /// returning a sequence builder with a capacity of at least `len`.
    ///
    /// This is intended to be used by I/O endpoints to allocate memory for I/O operations.
    /// If you need to create/fill I/O buffers in user code, you should request the memory
    /// directly from the I/O endpoint you are using, typically via an implementation of
    /// [`ProvideMemory`][crate::mem::ProvideMemory] provided by the I/O endpoint.
    #[must_use]
    pub fn reserve(&self, len: usize, _options: ReserveOptions) -> SequenceBuilder {
        // Placeholder until we actually implement some memory management options.
        const PREFERRED_BLOCK_SIZE: NonZero<usize> = nz!(12345);
        self.resources.memory_pool().rent(len, PREFERRED_BLOCK_SIZE)
    }

    /// Executes a caller-defined system task via the runtime environment that the I/O subsystem
    /// is configured to use. The result will be returned asynchronously to the caller.
    ///
    /// This is a convenience function to enable callers to schedule system tasks without having to
    /// maintain their own separate references to the runtime environment, as the I/O context needs
    /// this functionality for its own use anyway.
    ///
    /// The task will be executed on an unspecified thread owned by the runtime environment.
    pub async fn execute_system_task<F, R>(&self, category: SystemTaskCategory, body: F) -> R
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        self.resources.execute_system_task(category, body).await
    }

    /// Enqueues a caller-defined system task on the runtime environment that the I/O subsystem
    /// is configured to use. Will not wait for the task to be executed.
    ///
    /// This is a convenience function to enable callers to schedule system tasks without having to
    /// maintain their own separate references to the runtime environment, as the I/O context needs
    /// this functionality for its own use anyway.
    ///
    /// The task will be executed on an unspecified thread owned by the runtime environment.
    pub fn enqueue_system_task<F>(&self, category: SystemTaskCategory, body: F)
    where
        F: FnOnce() + Send + 'static,
    {
        self.resources.enqueue_system_task(category, body);
    }

    #[cfg(test)]
    pub(crate) const fn resources(&self) -> &Arc<Resources> {
        &self.resources
    }
}

#[cfg_attr(test, mutants::skip)]
impl ProvideMemory for Context {
    fn reserve(&self, min_bytes: usize) -> SequenceBuilder {
        self.reserve(min_bytes, ReserveOptions::default())
    }
}

#[cfg(test)]
#[cfg(not(target_os = "linux"))] // Linux is not yet supported, just enough to check it compiles.
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;
    use crate::AsNativePrimitivePrivate;
    use crate::pal::{MockPlatform, MockPrimitive, PlatformFacade};
    use crate::testing::{
        IoPumpMode, SimulatedCompletionQueue, use_default_memory_pool, with_io_test_harness_ex,
        with_partial_io_test_harness_and_platform,
    };

    #[test]
    fn handle_binding_round_trip_and_free() {
        // We send a platform-specific handle into ContextCore::bind() and get a bound handle back.
        // We expect to be able to project the returned object back into an original platform-
        // specific handle. We expect the PAL handle to be released once we drop the bound handle.

        let mut platform = MockPlatform::new();

        use_default_memory_pool::<1234>(&mut platform);

        platform
            .expect_new_completion_queue()
            .times(1)
            .returning(|| SimulatedCompletionQueue::new_disconnected().into());

        let platform = PlatformFacade::from_mock(platform);

        let handle_closed = Arc::new(AtomicBool::new(false));

        with_partial_io_test_harness_and_platform(platform, {
            let handle_closed = Arc::clone(&handle_closed);

            async move |test_harness| {
                // This is our original platform-specific handle we pass into the I/O subsystem for binding.
                let mut original_primitive = MockPrimitive::new();

                original_primitive
                    .expect_as_raw()
                    .times(1)
                    .return_const(1234_u64);

                original_primitive.expect_clone().times(1).returning({
                    let handle_closed = Arc::clone(&handle_closed);

                    move || {
                        // At end of life, the I/O subsystem will clone the primitive so it can be closed
                        // asynchronously on some unrelated worker thread, where it is out of the way.
                        let mut original_primitive_clone = MockPrimitive::new();
                        original_primitive_clone.expect_close().times(1).returning({
                            let handle_closed = Arc::clone(&handle_closed);

                            move || {
                                handle_closed.store(true, Ordering::Relaxed);
                            }
                        });

                        original_primitive_clone
                    }
                });

                // With ordinary primitives we can just pass them directly to the Context but here
                // we have to manually create an UnboundPrimitive because we operate on the ContextCore.
                let unbound_primitive = UnboundPrimitive::from_mock(original_primitive);

                let bound_primitive = test_harness
                    .context
                    .bind_primitive(unbound_primitive)
                    .unwrap();

                // Did we get back what we were expecting?
                let canary = bound_primitive.as_pal_primitive().as_mock().as_raw();
                assert_eq!(canary, 1234);

                // We have to drop the primitive here (or the driver will wait for it forever).
                drop(bound_primitive);

                // The test harness will perform the safe shutdown process.
            }
        });

        // We have an expectation of close() defined but that expectation merely verifies that it
        // happens eventually. However, we have a stronger requirement here: primitives must be
        // closed by the time the driver is dropped, they are part of the same shutdown to avoid
        // leaks (because there is no guarantee that the runtime keeps running after driver drop!).
        assert!(handle_closed.load(Ordering::Relaxed));
    }

    #[cfg(not(miri))] // Miri does not support real I/O.
    #[test]
    fn system_tasks_are_enqueued() {
        with_io_test_harness_ex(None, IoPumpMode::Always, async move |harness| {
            let result = harness
                .context
                .execute_system_task(SystemTaskCategory::Default, || 42)
                .await;

            assert_eq!(result, 42);

            // The "enqueue" variant does not tell us the result, so we set up our own signal.
            let (done_tx, done_rx) = oneshot::channel();

            harness
                .context
                .enqueue_system_task(SystemTaskCategory::Default, {
                    move || {
                        _ = done_tx.send(44);
                    }
                });

            let result = done_rx.await.unwrap();
            assert_eq!(result, 44);
        });
    }
}