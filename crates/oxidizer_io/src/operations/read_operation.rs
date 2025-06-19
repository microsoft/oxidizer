// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::{Arc, Weak};

use bytes::BufMut;
use negative_impl::negative_impl;
use tracing::{Level, event};

use crate::mem::{SequenceBuilder, SequenceBuilderAvailableIterator};
use crate::pal::ElementaryOperationKey;
use crate::{BeginResult, BoundPrimitiveRef, Operation, Resources, UserResource};

/// An I/O operation that reads some bytes of data by issuing a
/// single elementary "read" operation to the operating system.
///
/// User code typically does not need to interact with this type directly, as it is primarily used
/// to build higher-level I/O endpoint types (e.g. file, socket, tcp server, ...).
///
/// # Vectored read support in native I/O APIs
///
/// Not every native API supports vectored I/O. Some APIs support it but only when certain
/// preconditions are met (e.g. aligned buffers). If the conditions are not suitable to use
/// vectored I/O, a typical implementation is to only read enough data to fill the first chunk
/// in the callback provided to `begin()`.
#[derive(Debug)]
pub struct ReadOperation {
    // This ensures that the primitive we are operating on is kept alive until we issue the I/O
    // to the operating system (assuming that it is even alive once we start the operation - there
    // may be a delay between the user enqueuing I/O and the operation actually starting).
    primitive: Weak<BoundPrimitiveRef>,

    offset: u64,
    user_resources: Option<Box<dyn UserResource>>,
    buffer: SequenceBuilder,
    resources: Arc<Resources>,
}

impl ReadOperation {
    /// # Panics
    ///
    /// Panics if the sequence builder has a capacity of 0. A read with no capacity is likely to be
    /// a programming error. If you truly need to read 0 bytes, model it as a control operation.
    pub(crate) fn new(
        primitive: Weak<BoundPrimitiveRef>,
        buffer: SequenceBuilder,
        resources: Arc<Resources>,
    ) -> Self {
        assert_ne!(
            buffer.capacity(),
            0,
            "a read operation requires more than 0 bytes of capacity"
        );

        Self {
            primitive,
            offset: 0,
            user_resources: None,
            buffer,
            resources,
        }
    }

    /// For seekable I/O primitives (e.g. files), sets the offset where the operation
    /// should be performed.
    ///
    /// An I/O endpoint implementation must not set an offset (i.e. not call this method)
    /// if the I/O primitive has no concept of seeking/offset. Even specifying a value of 0
    /// indicates that a concept of "offset" is recognized by the I/O primitive.
    #[must_use]
    pub const fn with_offset(mut self, offset: u64) -> Self {
        self.offset = offset;
        self
    }

    /// Attaches an arbitrary value to the operation, to be dropped once the operation completes.
    ///
    /// This can be used to ensure some resources remain alive while the operation is in progress,
    /// even if the future driving the operation is dropped.
    #[must_use]
    pub fn with_resources(mut self, resources: impl UserResource) -> Self {
        self.user_resources = Some(Box::new(resources));
        self
    }

    /// Begins an asynchronous I/O operation by performing exactly one asynchronous system call
    /// in the callback provided to this method.
    ///
    /// The callback must perform exactly one system call, and return a `BeginResult` indicating
    /// whether the operation completed synchronously or was scheduled for asynchronous completion.
    ///
    /// The callback may be called at any point up until the returned future completes. In
    /// particular, it is not guaranteed to be called during `begin()` and not even guaranteed
    /// to be called on the same thread.
    ///
    /// If the primitive is closed before the future completes, the operation may be canceled and
    /// the callback may never get called.
    ///
    /// If the operation succeeds, the output will be a tuple with the number of bytes read and
    /// the same `SequenceBuilder` that was used to create the operation, now with the read
    /// bytes appended to it.
    ///
    /// # Panics
    ///
    /// Panics if the callback was called but it never requested the platform-specific system call
    /// parameters from the `ReadOperationArgs` struct, indicating that it did not perform any
    /// system call as was required.
    pub async fn begin<C>(mut self, callback: C) -> Result<(usize, SequenceBuilder), crate::Error>
    where
        C: for<'operation, 'callback> FnOnce(
                &'callback BoundPrimitiveRef,
                ReadOperationArgs<'operation, 'callback>,
            ) -> BeginResult<()>
            + Send
            + 'static,
    {
        // Note: in the future, all of this logic here may be deferred and batched. The current
        // implementation does it all immediately but while still valid, this is a special case.

        // We keep the primitive alive until we start the operation. Once the operation has
        // started (i.e. once the callback has been called), we no longer need to keep it alive
        // and drop this to allow cleanup if this operation is the last one keeping it alive,
        // at which point the OS may simply cancel the operation itself (which is fine).
        let Some(primitive) = self.primitive.upgrade() else {
            // The primitive was already closed before we started the operation.
            return Err(crate::Error::Canceled);
        };

        let usable_remaining = self.buffer.remaining_mut();

        // We constrain the capacity we are willing to use to u32::MAX because the operating
        // system APIs can only handle this many bytes per elementary operation.
        //
        // As we are dealing with a read operation, which has "at most" behavior, we cannot
        // automatically re-issue the operation to read more data because it may be valid and
        // intentional to terminate the read at any point - it depends on the caller's logic.
        let usable_remaining = usable_remaining.min(u32::MAX as usize);

        // We are reading bytes by writing them into a SequenceBuilder, which is a "read"
        // operation in terms of I/O primitive but a "write" in terms of SequenceBuilder.
        //
        // Even if the underlying I/O primitive does not support vectored I/O, we still treat it
        // as vectored - the I/O subsystem does not know if the callback is capable of filling
        // multiple chunks sequentially, so we just assume it is. If not, the 2nd and further
        // chunks will simply be ignored by the callback and remain empty.
        let mut vectored_write = self
            .buffer
            .begin_vectored_write_checked(Some(usable_remaining))
            .expect("guarded by usable_remaining logic above");

        // This guard will keep the I/O blocks alive until the operation completes, ensuring
        // that the memory remains valid even if the caller abandons this operation and drops the
        // future (releasing the SequenceBuilder we keep in the state machine). From Rust code,
        // the SequenceBuilder (and the SpanBuilder inside) have exclusive access, so we can be sure
        // that even if the Rust side is dropped, no concurrent access from Rust will occur while
        // the operating system is using the referenced memory in these blocks. Note that the I/O
        // blocks may also include memory not covered by our SpanBuilder instances, which is
        // accessed independently and out of our jurisdiction.
        let memory_guard = vectored_write.extend_lifetime();

        // This is where the callback will write any read bytes.
        let chunks_iter = vectored_write.iter_chunks_mut();

        let mut operation = Operation::new(
            self.offset,
            memory_guard,
            self.user_resources.take(),
            self.resources.pal(),
        );

        event!(
            Level::TRACE,
            message = "elementary begin",
            key = operation.elementary_operation_key().0
        );

        let result_rx = operation.take_result_rx();
        let elementary_operation_key = operation.elementary_operation_key();

        let mut bytes_read_synchronously: u32 = 0;

        let args = ReadOperationArgs {
            chunks_iter,
            elementary_operation_key: Some(elementary_operation_key),
            bytes_read: &mut bytes_read_synchronously,
        };

        let synchronous_result = callback(&primitive, args);

        // The operation has started (or synchronously completed) and we no longer have
        // any need to keep the primitive alive (especially if we are going to suspend and await).
        // If someone cares about the operation completing asynchronously, they need to
        // keep the primitive alive themselves.
        drop(primitive);

        match synchronous_result {
            BeginResult::Asynchronous => {
                // Everything is going well, now we just need to wire up to await the result.
                event!(
                    Level::TRACE,
                    message = "elementary async",
                    key = elementary_operation_key.0,
                );
            }
            BeginResult::CompletedSynchronously(result) => {
                event!(
                    Level::TRACE,
                    message = "elementary immediate",
                    result = ?result,
                    key = elementary_operation_key.0,
                    bytes_read_synchronously
                );

                return match result {
                    Ok(()) => {
                        // The operation completed synchronously. This means we will not get a
                        // completion notification and must handle the result inline.

                        let bytes_read_synchronously_usize = usize::try_from(bytes_read_synchronously)
                            .expect("the operation uses a buffer constrained to <= usize::MAX, so it cannot transfer more than usize::MAX");

                        // If the number of bytes read went over our capacity, we have
                        // overflowed a buffer (e.g. because syscall arguments were improperly
                        // prepared) and it is no longer safe to continue process execution.
                        assert!(bytes_read_synchronously_usize <= usable_remaining);

                        // SAFETY: We have to ensure that the bytes really were initialized with
                        // data. The callback says they were and we have no reason not to trust it.
                        unsafe {
                            vectored_write.commit(bytes_read_synchronously_usize);
                        }

                        Ok((bytes_read_synchronously_usize, self.buffer))
                    }
                    Err(e) => {
                        // Something went wrong. In this case, the elementary operation
                        // was not written by the OS and no completion notification will
                        // be received by the completion queue.
                        Err(e)
                    }
                };
            }
        }

        // We register the operation in the resource set so that the I/O driver
        // can find it once we receive the completion notification.
        self.resources
            .operations_mut()
            .insert(elementary_operation_key, operation);

        let bytes_read = result_rx
            .await
            .expect("I/O driver side of an I/O operation vanished while awaiting result")?;

        let bytes_read_usize = usize::try_from(bytes_read)
            .expect("the operation uses a buffer constrained to <= usize::MAX, so it cannot transfer more than usize::MAX");

        // If the number of bytes read went over our capacity, we have
        // overflowed a buffer (e.g. because syscall arguments were improperly
        // prepared) and it is no longer safe to continue process execution.
        assert!(bytes_read_usize <= usable_remaining);

        // SAFETY: We have to ensure that the bytes really were initialized with
        // data. The callback says they were and we have no reason not to trust it.
        unsafe {
            vectored_write.commit(bytes_read_usize);
        }

        Ok((bytes_read_usize, self.buffer))
    }
}

/// Arguments provided by the I/O subsystem to the `begin()` callback that executes an elementary
/// I/O operation to induce the operating system to read some bytes of data.
///
/// # Resource management
///
/// References with the `'operation` lifetime are valid for the entire duration of the elementary
/// operation, up until the operating system notifies us that the operation has been completed.
///
/// This means the references remain valid even when moved out of the domain of the Rust borrow
/// checker as pointers, and may be handed to the operating system. The full I/O operation
/// lifetime does not map to a single Rust lifetime - the `'operation` lifetime is merely a marker.
///
/// References with the `'callback` lifetime are valid for the duration of the callback
/// that provides this arguments object.
#[derive(Debug)]
pub struct ReadOperationArgs<'operation, 'callback> {
    chunks_iter: SequenceBuilderAvailableIterator<'operation>,
    bytes_read: &'callback mut u32,

    // This is Option because it can only be called once (to consume it) because each `begin()` is
    // only intended to start a single system call. We use the Option to protect against double-get
    // and to ensure (via Drop check) that the caller does consume it (if not - no syscall made?!)
    //
    // Of course, even if the callback asks for the system call parameters exactly once, there is
    // no guarantee we can get here that it will use the parameters to actually make a system call
    // but if it does not, that is merely a memory leak and not a safety issue, so good enough.
    elementary_operation_key: Option<ElementaryOperationKey>,
}

impl<'operation> ReadOperationArgs<'operation, '_> {
    /// Iterate over the chunks of memory that may be filled by the elementary I/O operation.
    ///
    /// The operation may fill the chunks in any amount (including producing 0 bytes) but must fill
    /// them in order and without leaving any gaps when the chunks are viewed as a concatenated
    /// sequence.
    pub const fn iter_chunks(&mut self) -> &mut SequenceBuilderAvailableIterator<'operation> {
        &mut self.chunks_iter
    }

    /// Gets an exclusive reference to a field that must be set to the number of bytes that were
    /// read by the operation if the operation completes synchronously, as signaled by
    /// a callback return value of [`BeginResult::CompletedSynchronously`].
    ///
    /// This field is ignored if the operation will complete asynchronously, as signaled by a
    /// callback return value of [`BeginResult::Asynchronous`].
    pub const fn bytes_read_synchronously_as_mut(&mut self) -> &mut u32 {
        self.bytes_read
    }

    /// # Panics
    ///
    /// Panics if called more than once.
    pub(crate) const fn consume_elementary_operation_key(&mut self) -> ElementaryOperationKey {
        self.elementary_operation_key
            .take()
            .expect("elementary operation consumed more than once from ReadOperationArgs")
    }
}

impl Drop for ReadOperationArgs<'_, '_> {
    fn drop(&mut self) {
        assert!(
            self.elementary_operation_key.is_none(),
            "system call parameters not consumed from ReadOperationArgs - no system call could have been made"
        );
    }
}

#[negative_impl]
impl !Send for ReadOperationArgs<'_, '_> {}
#[negative_impl]
impl !Sync for ReadOperationArgs<'_, '_> {}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::indexing_slicing,
        reason = "fine in tests - we want as many panics as we can get"
    )]

    use std::io::ErrorKind;
    use std::pin::pin;
    use std::task::{Context, Poll};

    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::*;
    use crate::pal::{MockPlatform, PlatformFacade};
    use crate::testing::{
        assert_panic, bind_dummy_primitive, expect_elementary_operations,
        new_failed_completion_notification, new_successful_completion_notification,
        use_default_memory_pool, use_simulated_completion_queue,
        with_partial_io_test_harness_and_platform,
    };
    use crate::{ERR_POISONED_LOCK, ReserveOptions};

    #[test]
    fn thread_mobile_type() {
        assert_impl_all!(ReadOperation: Send);
    }

    #[test]
    fn args_is_single_threaded_type() {
        assert_not_impl_any!(ReadOperationArgs: Send, Sync);
    }

    #[test]
    fn read_called_with_zero_capacity_panics() {
        let mut platform = MockPlatform::new();

        use_default_memory_pool::<1234>(&mut platform);
        use_simulated_completion_queue(&mut platform);

        let platform = PlatformFacade::from_mock(platform);

        with_partial_io_test_harness_and_platform(platform, async move |test_harness| {
            let primitive = bind_dummy_primitive(&test_harness.context);

            let buffer = SequenceBuilder::new();
            assert_panic!(primitive.read_bytes(buffer));
        });
    }

    #[test]
    fn read_yields_zero_bytes_async() {
        let mut platform = MockPlatform::new();

        use_default_memory_pool::<1234>(&mut platform);
        expect_elementary_operations(1, &mut platform);

        let queue_simulation = use_simulated_completion_queue(&mut platform);
        let platform = PlatformFacade::from_mock(platform);

        with_partial_io_test_harness_and_platform(platform, async move |test_harness| {
            let primitive = bind_dummy_primitive(&test_harness.context);

            let buffer = test_harness.context.reserve(100, ReserveOptions::default());

            let read_future = primitive
                .read_bytes(buffer)
                .begin(move |_primitive, mut args| {
                    queue_simulation.completed.lock().unwrap().push_back(
                        new_successful_completion_notification(
                            args.consume_elementary_operation_key(),
                            0,
                        ),
                    );

                    BeginResult::Asynchronous
                });
            let mut read_future = pin!(read_future);

            let mut cx = Context::from_waker(futures::task::noop_waker_ref());
            let poll_result = read_future.as_mut().poll(&mut cx);
            assert!(matches!(poll_result, Poll::Pending));

            // We expect this to process the completion which is already in the queue.
            test_harness.driver.borrow_mut().process_completions(0);
            let mut cx = Context::from_waker(futures::task::noop_waker_ref());
            let poll_result = read_future.as_mut().poll(&mut cx);

            match poll_result {
                Poll::Ready(Ok((bytes_read, buffer))) => {
                    assert_eq!(bytes_read, 0);
                    assert_eq!(buffer.len(), 0);
                }
                _ => panic!("unexpected poll result: {poll_result:?}"),
            }
        });
    }

    #[test]
    fn read_yields_zero_bytes_sync() {
        let mut platform = MockPlatform::new();

        use_default_memory_pool::<1234>(&mut platform);
        expect_elementary_operations(1, &mut platform);

        use_simulated_completion_queue(&mut platform);

        let platform = PlatformFacade::from_mock(platform);

        with_partial_io_test_harness_and_platform(platform, async move |test_harness| {
            let primitive = bind_dummy_primitive(&test_harness.context);

            let buffer = test_harness.context.reserve(100, ReserveOptions::default());

            let read_future = primitive
                .read_bytes(buffer)
                .begin(move |_primitive, mut args| {
                    // We must consume the elementary operation key to indicate that we have made
                    // (or simulated) a system call, because this is mandatory in real code.
                    args.consume_elementary_operation_key();

                    *args.bytes_read_synchronously_as_mut() = 0;
                    BeginResult::CompletedSynchronously(Ok(()))
                });
            let mut read_future = pin!(read_future);

            let mut cx = Context::from_waker(futures::task::noop_waker_ref());
            let poll_result = read_future.as_mut().poll(&mut cx);

            match poll_result {
                Poll::Ready(Ok((bytes_read, buffer))) => {
                    assert_eq!(bytes_read, 0);
                    assert_eq!(buffer.len(), 0);
                }
                _ => panic!("unexpected poll result: {poll_result:?}"),
            }
        });
    }

    #[test]
    fn read_into_single_block_async() {
        let mut platform = MockPlatform::new();

        use_default_memory_pool::<1234>(&mut platform);
        expect_elementary_operations(1, &mut platform);

        let queue_simulation = use_simulated_completion_queue(&mut platform);

        let platform = PlatformFacade::from_mock(platform);

        with_partial_io_test_harness_and_platform(platform, async move |test_harness| {
            let primitive = bind_dummy_primitive(&test_harness.context);

            // Requesting 100, will get 1234.
            let buffer = test_harness.context.reserve(100, ReserveOptions::default());

            let read_future = primitive
                .read_bytes(buffer)
                .begin(move |_primitive, mut args| {
                    let chunks = args.iter_chunks();
                    let first_chunk = chunks.next().unwrap();
                    assert_eq!(first_chunk.len(), 1234);

                    let second_chunk = chunks.next();
                    assert!(second_chunk.is_none());

                    // We requested 100 but we got 1234 so we are allowed to use more.
                    // Let's say we used 500 here.
                    queue_simulation.completed.lock().unwrap().push_back(
                        new_successful_completion_notification(
                            args.consume_elementary_operation_key(),
                            500,
                        ),
                    );

                    BeginResult::Asynchronous
                });
            let mut read_future = pin!(read_future);

            let mut cx = Context::from_waker(futures::task::noop_waker_ref());
            let poll_result = read_future.as_mut().poll(&mut cx);
            assert!(matches!(poll_result, Poll::Pending));

            // We expect this to process the completion which is already in the queue.
            test_harness.driver.borrow_mut().process_completions(0);

            let mut cx = Context::from_waker(futures::task::noop_waker_ref());
            let poll_result = read_future.as_mut().poll(&mut cx);

            match poll_result {
                Poll::Ready(Ok((bytes_read, buffer))) => {
                    assert_eq!(bytes_read, 500);
                    assert_eq!(buffer.len(), 500);
                }
                _ => panic!("unexpected poll result: {poll_result:?}"),
            }
        });
    }

    #[test]
    fn read_into_single_block_part_filled_async() {
        let mut platform = MockPlatform::new();

        use_default_memory_pool::<1234>(&mut platform);
        expect_elementary_operations(1, &mut platform);

        let queue_simulation = use_simulated_completion_queue(&mut platform);

        let platform = PlatformFacade::from_mock(platform);

        with_partial_io_test_harness_and_platform(platform, async move |test_harness| {
            let primitive = bind_dummy_primitive(&test_harness.context);

            // Requesting 100, will get 1234.
            let mut buffer = test_harness.context.reserve(100, ReserveOptions::default());

            // First 50 bytes are already filled.
            buffer.put_bytes(99, 50);

            let read_future = primitive
                .read_bytes(buffer)
                .begin(move |_primitive, mut args| {
                    let chunks = args.iter_chunks();
                    let first_chunk = chunks.next().unwrap();
                    assert_eq!(first_chunk.len(), 1234 - 50);

                    let second_chunk = chunks.next();
                    assert!(second_chunk.is_none());

                    // We requested 100 but we got (1234 - 50) so we are allowed to use more.
                    // Let's say we used 500 here.
                    queue_simulation.completed.lock().unwrap().push_back(
                        new_successful_completion_notification(
                            args.consume_elementary_operation_key(),
                            500,
                        ),
                    );

                    BeginResult::Asynchronous
                });
            let mut read_future = pin!(read_future);

            let mut cx = Context::from_waker(futures::task::noop_waker_ref());
            let poll_result = read_future.as_mut().poll(&mut cx);
            assert!(matches!(poll_result, Poll::Pending));

            // We expect this to process the completion which is already in the queue.
            test_harness.driver.borrow_mut().process_completions(0);

            let mut cx = Context::from_waker(futures::task::noop_waker_ref());
            let poll_result = read_future.as_mut().poll(&mut cx);

            match poll_result {
                Poll::Ready(Ok((bytes_read, buffer))) => {
                    assert_eq!(bytes_read, 500);
                    assert_eq!(buffer.len(), 550);
                }
                _ => panic!("unexpected poll result: {poll_result:?}"),
            }
        });
    }
    #[test]
    fn read_into_multi_block_async() {
        let mut platform = MockPlatform::new();

        use_default_memory_pool::<100>(&mut platform);
        expect_elementary_operations(1, &mut platform);

        let queue_simulation = use_simulated_completion_queue(&mut platform);

        let platform = PlatformFacade::from_mock(platform);

        with_partial_io_test_harness_and_platform(platform, async move |test_harness| {
            let primitive = bind_dummy_primitive(&test_harness.context);

            // Requesting 150, will get 200.
            let buffer = test_harness.context.reserve(150, ReserveOptions::default());

            let read_future = primitive
                .read_bytes(buffer)
                .begin(move |_primitive, mut args| {
                    let chunks = args.iter_chunks();
                    let first_chunk = chunks.next().unwrap();
                    assert_eq!(first_chunk.len(), 100);

                    let second_chunk = chunks.next().unwrap();
                    assert_eq!(second_chunk.len(), 100);

                    let third_chunk = chunks.next();
                    assert!(third_chunk.is_none());

                    // We requested 150 but we got 200 so we are allowed to use more.
                    // Let's say we used 199 here.
                    queue_simulation.completed.lock().unwrap().push_back(
                        new_successful_completion_notification(
                            args.consume_elementary_operation_key(),
                            199,
                        ),
                    );

                    BeginResult::Asynchronous
                });
            let mut read_future = pin!(read_future);

            let mut cx = Context::from_waker(futures::task::noop_waker_ref());
            let poll_result = read_future.as_mut().poll(&mut cx);
            assert!(matches!(poll_result, Poll::Pending));

            // We expect this to process the completion which is already in the queue.
            test_harness.driver.borrow_mut().process_completions(0);

            let mut cx = Context::from_waker(futures::task::noop_waker_ref());
            let poll_result = read_future.as_mut().poll(&mut cx);

            match poll_result {
                Poll::Ready(Ok((bytes_read, buffer))) => {
                    assert_eq!(bytes_read, 199);
                    assert_eq!(buffer.len(), 199);
                }
                _ => panic!("unexpected poll result: {poll_result:?}"),
            }
        });
    }
    #[test]
    fn read_into_multi_block_part_filled_async() {
        let mut platform = MockPlatform::new();

        use_default_memory_pool::<100>(&mut platform);
        expect_elementary_operations(1, &mut platform);

        let queue_simulation = use_simulated_completion_queue(&mut platform);

        let platform = PlatformFacade::from_mock(platform);

        with_partial_io_test_harness_and_platform(platform, async move |test_harness| {
            let primitive = bind_dummy_primitive(&test_harness.context);

            // Requesting 250, will get 300.
            let mut buffer = test_harness.context.reserve(250, ReserveOptions::default());

            // First 110 bytes are already filled.
            buffer.put_bytes(99, 110);

            let read_future = primitive
                .read_bytes(buffer)
                .begin(move |_primitive, mut args| {
                    // We do not see already filled chunk.
                    // We see the 90 remaining in 2nd, and the 100 remaining in 3rd.
                    let chunks = args.iter_chunks();
                    let first_chunk = chunks.next().unwrap();
                    assert_eq!(first_chunk.len(), 90);

                    let second_chunk = chunks.next().unwrap();
                    assert_eq!(second_chunk.len(), 100);

                    let third_chunk = chunks.next();
                    assert!(third_chunk.is_none());

                    // We are allowed to use all the capacity we have (total 190).
                    // Let's say we used 189 here.
                    queue_simulation.completed.lock().unwrap().push_back(
                        new_successful_completion_notification(
                            args.consume_elementary_operation_key(),
                            189,
                        ),
                    );

                    BeginResult::Asynchronous
                });
            let mut read_future = pin!(read_future);

            let mut cx = Context::from_waker(futures::task::noop_waker_ref());
            let poll_result = read_future.as_mut().poll(&mut cx);
            assert!(matches!(poll_result, Poll::Pending));

            // We expect this to process the completion which is already in the queue.
            test_harness.driver.borrow_mut().process_completions(0);

            let mut cx = Context::from_waker(futures::task::noop_waker_ref());
            let poll_result = read_future.as_mut().poll(&mut cx);

            match poll_result {
                Poll::Ready(Ok((bytes_read, buffer))) => {
                    assert_eq!(bytes_read, 189);
                    assert_eq!(buffer.len(), 299);
                }
                _ => panic!("unexpected poll result: {poll_result:?}"),
            }
        });
    }
    #[test]
    fn read_into_multi_block_sync() {
        let mut platform = MockPlatform::new();

        use_default_memory_pool::<100>(&mut platform);
        expect_elementary_operations(1, &mut platform);

        use_simulated_completion_queue(&mut platform);

        let platform = PlatformFacade::from_mock(platform);

        with_partial_io_test_harness_and_platform(platform, async move |test_harness| {
            let primitive = bind_dummy_primitive(&test_harness.context);

            // Requesting 150, will get 200.
            let buffer = test_harness.context.reserve(150, ReserveOptions::default());

            let read_future = primitive
                .read_bytes(buffer)
                .begin(move |_primitive, mut args| {
                    let chunks = args.iter_chunks();
                    let first_chunk = chunks.next().unwrap();
                    assert_eq!(first_chunk.len(), 100);

                    let second_chunk = chunks.next().unwrap();
                    assert_eq!(second_chunk.len(), 100);

                    let third_chunk = chunks.next();
                    assert!(third_chunk.is_none());

                    // We must consume the elementary operation key to indicate that we have made
                    // (or simulated) a system call, because this is mandatory in real code.
                    args.consume_elementary_operation_key();

                    // We requested 150 but we got 200 so we are allowed to use more.
                    // Let's say we used 199 here.
                    *args.bytes_read_synchronously_as_mut() = 199;
                    BeginResult::CompletedSynchronously(Ok(()))
                });
            let mut read_future = pin!(read_future);

            let mut cx = Context::from_waker(futures::task::noop_waker_ref());
            let poll_result = read_future.as_mut().poll(&mut cx);

            match poll_result {
                Poll::Ready(Ok((bytes_read, buffer))) => {
                    assert_eq!(bytes_read, 199);
                    assert_eq!(buffer.len(), 199);
                }
                _ => panic!("unexpected poll result: {poll_result:?}"),
            }
        });
    }

    #[test]
    #[cfg(not(miri))]
    fn read_constrained_to_u32_max() {
        // If a read above u32::MAX is requested, the callback only sees a request for u32::MAX.
        // This is because the OS APIs only support u32::MAX as the maximum size of a read, so
        // we mirror this limitation. It is not practical to automatically extend reads the same
        // way as writes can be extended, so we just interpret it as a hard cap on an operation.
        // Passing more than u32::MAX of buffers into a single read is useless, in other words.

        // The most straightforward way to test this is to really allocate a giant buffer.
        const HUNDRED_MEGABYTES: usize = 100 * 1024 * 1024;
        const TOTAL_BYTES: usize = 5 * 1024 * 1024 * 1024;

        let mut platform = MockPlatform::new();

        use_default_memory_pool::<HUNDRED_MEGABYTES>(&mut platform);
        expect_elementary_operations(1, &mut platform);
        use_simulated_completion_queue(&mut platform);

        let platform = PlatformFacade::from_mock(platform);

        with_partial_io_test_harness_and_platform(platform, async move |test_harness| {
            let primitive = bind_dummy_primitive(&test_harness.context);

            let buffer = test_harness
                .context
                .reserve(TOTAL_BYTES, ReserveOptions::default());

            let read_future = primitive
                .read_bytes(buffer)
                .begin(move |_primitive, mut args| {
                    let visible_buffer_size = args.iter_chunks().map(|c| c.len()).sum::<usize>();
                    assert_eq!(visible_buffer_size, u32::MAX as usize);

                    // Pretend we are doing a real system call.
                    args.consume_elementary_operation_key();

                    *args.bytes_read_synchronously_as_mut() =
                        u32::try_from(visible_buffer_size).unwrap();
                    BeginResult::CompletedSynchronously(Ok(()))
                });
            let mut read_future = pin!(read_future);

            let mut cx = Context::from_waker(futures::task::noop_waker_ref());
            let (bytes_read, buffer) = match read_future.as_mut().poll(&mut cx) {
                Poll::Ready(x) => x.unwrap(),
                Poll::Pending => panic!("this test is sync, should be no async activity"),
            };

            assert_eq!(bytes_read, u32::MAX as usize);
            assert_eq!(buffer.len(), u32::MAX as usize);
        });
    }

    #[test]
    fn read_failed_async() {
        let mut platform = MockPlatform::new();

        use_default_memory_pool::<1234>(&mut platform);
        expect_elementary_operations(1, &mut platform);

        let queue_simulation = use_simulated_completion_queue(&mut platform);

        let platform = PlatformFacade::from_mock(platform);

        with_partial_io_test_harness_and_platform(platform, async move |test_harness| {
            let primitive = bind_dummy_primitive(&test_harness.context);

            let buffer = test_harness.context.reserve(100, ReserveOptions::default());

            let read_future = primitive
                .read_bytes(buffer)
                .begin(move |_primitive, mut args| {
                    queue_simulation.completed.lock().unwrap().push_back(
                        new_failed_completion_notification(args.consume_elementary_operation_key()),
                    );

                    BeginResult::Asynchronous
                });
            let mut read_future = pin!(read_future);

            let mut cx = Context::from_waker(futures::task::noop_waker_ref());
            let poll_result = read_future.as_mut().poll(&mut cx);
            assert!(matches!(poll_result, Poll::Pending));

            // We expect this to process the completion which is already in the queue.
            test_harness.driver.borrow_mut().process_completions(0);

            let mut cx = Context::from_waker(futures::task::noop_waker_ref());
            let poll_result = read_future.as_mut().poll(&mut cx);
            // We do not have an API contract for what error it must be.
            assert!(matches!(poll_result, Poll::Ready(Err(_))));
        });
    }
    #[test]
    fn read_failed_sync() {
        let mut platform = MockPlatform::new();

        use_default_memory_pool::<1234>(&mut platform);
        expect_elementary_operations(1, &mut platform);

        use_simulated_completion_queue(&mut platform);

        let platform = PlatformFacade::from_mock(platform);

        with_partial_io_test_harness_and_platform(platform, async move |test_harness| {
            let primitive = bind_dummy_primitive(&test_harness.context);

            let buffer = test_harness.context.reserve(100, ReserveOptions::default());

            let read_future = primitive
                .read_bytes(buffer)
                .begin(move |_primitive, mut args| {
                    // We must consume the elementary operation key to indicate that we have made
                    // (or simulated) a system call, because this is mandatory in real code.
                    args.consume_elementary_operation_key();

                    BeginResult::CompletedSynchronously(Err(std::io::Error::new(
                        ErrorKind::AlreadyExists,
                        "something went wrong".to_string(),
                    )
                    .into()))
                });
            let mut read_future = pin!(read_future);

            let mut cx = Context::from_waker(futures::task::noop_waker_ref());
            let poll_result = read_future.as_mut().poll(&mut cx);
            // We do not have an API contract for what error it must be.
            assert!(matches!(poll_result, Poll::Ready(Err(_))));
        });
    }
    #[test]
    fn read_went_over_capacity_async() {
        let mut platform = MockPlatform::new();

        use_default_memory_pool::<100>(&mut platform);
        expect_elementary_operations(1, &mut platform);

        let queue_simulation = use_simulated_completion_queue(&mut platform);

        let platform = PlatformFacade::from_mock(platform);

        with_partial_io_test_harness_and_platform(platform, async move |test_harness| {
            let primitive = bind_dummy_primitive(&test_harness.context);

            // Requesting 100, got 100.
            let buffer = test_harness.context.reserve(100, ReserveOptions::default());

            let read_future = primitive
                .read_bytes(buffer)
                .begin(move |_primitive, mut args| {
                    let chunks = args.iter_chunks();
                    let first_chunk = chunks.next().unwrap();
                    assert_eq!(first_chunk.len(), 100);

                    let second_chunk = chunks.next();
                    assert!(second_chunk.is_none());

                    // We have a capacity of 100 but somehow the operation read 101 bytes!
                    queue_simulation.completed.lock().unwrap().push_back(
                        new_successful_completion_notification(
                            args.consume_elementary_operation_key(),
                            101,
                        ),
                    );

                    BeginResult::Asynchronous
                });
            let mut read_future = pin!(read_future);

            let mut cx = Context::from_waker(futures::task::noop_waker_ref());
            let poll_result = read_future.as_mut().poll(&mut cx);
            assert!(matches!(poll_result, Poll::Pending));

            // We expect this to process the completion which is already in the queue.
            test_harness.driver.borrow_mut().process_completions(0);

            let mut cx = Context::from_waker(futures::task::noop_waker_ref());

            // Expecting a panic from here.
            assert_panic!(_ = read_future.as_mut().poll(&mut cx));
        });
    }
    #[test]
    fn read_went_over_capacity_sync() {
        let mut platform = MockPlatform::new();

        use_default_memory_pool::<100>(&mut platform);
        expect_elementary_operations(1, &mut platform);

        use_simulated_completion_queue(&mut platform);

        let platform = PlatformFacade::from_mock(platform);

        with_partial_io_test_harness_and_platform(platform, async move |test_harness| {
            let primitive = bind_dummy_primitive(&test_harness.context);

            // Requesting 100, got 100.
            let buffer = test_harness.context.reserve(100, ReserveOptions::default());

            let read_future = primitive
                .read_bytes(buffer)
                .begin(move |_primitive, mut args| {
                    let chunks = args.iter_chunks();
                    let first_chunk = chunks.next().unwrap();
                    assert_eq!(first_chunk.len(), 100);

                    let second_chunk = chunks.next();
                    assert!(second_chunk.is_none());

                    // We must consume the elementary operation key to indicate that we have made
                    // (or simulated) a system call, because this is mandatory in real code.
                    args.consume_elementary_operation_key();

                    *args.bytes_read_synchronously_as_mut() = 101;
                    BeginResult::CompletedSynchronously(Ok(()))
                });
            let mut read_future = pin!(read_future);

            let mut cx = Context::from_waker(futures::task::noop_waker_ref());

            // Expecting a panic from here.
            assert_panic!(_ = read_future.as_mut().poll(&mut cx));
        });
    }
    #[test]
    fn read_offset_reached_platform() {
        let mut platform = MockPlatform::new();

        use_default_memory_pool::<1234>(&mut platform);
        let operation_offsets = expect_elementary_operations(1, &mut platform);

        let queue_simulation = use_simulated_completion_queue(&mut platform);

        let platform = PlatformFacade::from_mock(platform);

        with_partial_io_test_harness_and_platform(platform, async move |test_harness| {
            let primitive = bind_dummy_primitive(&test_harness.context);

            // Requesting 100, will get 1234.
            let buffer = test_harness.context.reserve(100, ReserveOptions::default());

            let read_future = primitive.read_bytes(buffer).with_offset(625_453).begin(
                move |_primitive, mut args| {
                    let chunks = args.iter_chunks();
                    let first_chunk = chunks.next().unwrap();
                    assert_eq!(first_chunk.len(), 1234);

                    let second_chunk = chunks.next();
                    assert!(second_chunk.is_none());

                    let elementary_operation_key = args.consume_elementary_operation_key();

                    // The contract of expect_elementary_operations() says the key is the index
                    // into the operation_offsets list.
                    assert_eq!(
                        operation_offsets.lock().expect(ERR_POISONED_LOCK)
                            [elementary_operation_key.0],
                        Some(625_453)
                    );

                    // We requested 100 but we got 1234 so we are allowed to use more.
                    // Let's say we used 500 here.
                    queue_simulation.completed.lock().unwrap().push_back(
                        new_successful_completion_notification(elementary_operation_key, 500),
                    );

                    BeginResult::Asynchronous
                },
            );
            let mut read_future = pin!(read_future);

            let mut cx = Context::from_waker(futures::task::noop_waker_ref());
            let poll_result = read_future.as_mut().poll(&mut cx);
            assert!(matches!(poll_result, Poll::Pending));

            // We expect this to process the completion which is already in the queue.
            test_harness.driver.borrow_mut().process_completions(0);

            let mut cx = Context::from_waker(futures::task::noop_waker_ref());
            let poll_result = read_future.as_mut().poll(&mut cx);

            match poll_result {
                Poll::Ready(Ok((bytes_read, buffer))) => {
                    assert_eq!(bytes_read, 500);
                    assert_eq!(buffer.len(), 500);
                }
                _ => panic!("unexpected poll result: {poll_result:?}"),
            }
        });
    }
    #[test]
    fn resources_kept_alive_after_future_dropped() {
        let resource = Arc::new(42);
        let weak = Arc::downgrade(&resource);

        let mut platform = MockPlatform::new();

        use_default_memory_pool::<1234>(&mut platform);
        expect_elementary_operations(1, &mut platform);

        let queue_simulation = use_simulated_completion_queue(&mut platform);
        let platform = PlatformFacade::from_mock(platform);

        with_partial_io_test_harness_and_platform(platform, async move |test_harness| {
            {
                let primitive = bind_dummy_primitive(&test_harness.context);

                let buffer = test_harness.context.reserve(100, ReserveOptions::default());

                let read_future = primitive.read_bytes(buffer).with_resources(resource).begin(
                    move |_primitive, mut args| {
                        queue_simulation.completed.lock().unwrap().push_back(
                            new_successful_completion_notification(
                                args.consume_elementary_operation_key(),
                                0,
                            ),
                        );

                        BeginResult::Asynchronous
                    },
                );
                let mut read_future = pin!(read_future);

                let mut cx = Context::from_waker(futures::task::noop_waker_ref());
                let poll_result = read_future.as_mut().poll(&mut cx);
                assert!(matches!(poll_result, Poll::Pending));
            }

            // The future has been dropped! However, we expect that our resource is still alive
            // because the operation is still asynchronously ongoing.
            assert!(weak.upgrade().is_some());

            // We expect this to process the completion which is already in the queue.
            test_harness.driver.borrow_mut().process_completions(0);

            // Now the operation has completed and the attached resources should be released.
            assert!(weak.upgrade().is_none());
        });
    }

    // We do not do proper cleanup of the test harness (as that may be impossible after a
    // panic), which makes Miri all bothered about resource leaks. So let's not run under Miri.
    #[cfg(not(miri))]
    #[test]
    #[should_panic]
    #[expect(
        clippy::should_panic_without_expect,
        reason = "TODO: Add an expected string to [should_panic]"
    )]
    fn read_non_syscall_panic() {
        let mut platform = MockPlatform::new();

        use_default_memory_pool::<1234>(&mut platform);
        expect_elementary_operations(1, &mut platform);

        use_simulated_completion_queue(&mut platform);

        let platform = PlatformFacade::from_mock(platform);

        with_partial_io_test_harness_and_platform(platform, |test_harness| async move {
            let primitive = bind_dummy_primitive(&test_harness.context);

            let buffer = test_harness.context.reserve(100, ReserveOptions::default());

            let read_future = primitive
                .read_bytes(buffer)
                // We must consume the elementary operation key, even if we don't use it, to
                // indicate that we have made (or simulated) a system call. We do not!
                .begin(|_, _| BeginResult::CompletedSynchronously(Ok(())));
            let mut read_future = pin!(read_future);

            let mut cx = Context::from_waker(futures::task::noop_waker_ref());

            // Should panic here.
            _ = read_future.as_mut().poll(&mut cx);
        });
    }

    // We do not do proper cleanup of the test harness (as that may be impossible after a
    // panic), which makes Miri all bothered about resource leaks. So let's not run under Miri.
    #[cfg(not(miri))]
    #[test]
    #[should_panic]
    #[expect(
        clippy::should_panic_without_expect,
        reason = "TODO: Add an expected string to [should_panic]"
    )]
    fn two_syscalls_panic() {
        let mut platform = MockPlatform::new();

        use_default_memory_pool::<1234>(&mut platform);
        expect_elementary_operations(1, &mut platform);

        use_simulated_completion_queue(&mut platform);

        let platform = PlatformFacade::from_mock(platform);

        with_partial_io_test_harness_and_platform(platform, |test_harness| async move {
            let primitive = bind_dummy_primitive(&test_harness.context);

            let buffer = test_harness.context.reserve(100, ReserveOptions::default());

            let read_future = primitive
                .read_bytes(buffer)
                .begin(move |_primitive, mut args| {
                    // Calling this twice is invalid, as it implies we are making two syscalls
                    // in a single callback, which is not allowed. The second call should panic.
                    args.consume_elementary_operation_key();
                    args.consume_elementary_operation_key();

                    BeginResult::CompletedSynchronously(Ok(()))
                });
            let mut read_future = pin!(read_future);

            let mut cx = Context::from_waker(futures::task::noop_waker_ref());

            // Should panic here.
            _ = read_future.as_mut().poll(&mut cx);
        });
    }
}