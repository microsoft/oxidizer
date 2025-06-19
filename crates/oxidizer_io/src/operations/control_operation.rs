// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::{Arc, Weak};

use negative_impl::negative_impl;
use tracing::{Level, event};

use crate::mem::MemoryGuard;
use crate::pal::ElementaryOperationKey;
use crate::{BeginResult, BoundPrimitiveRef, Operation, Resources, UserResource};

/// An I/O operation that executes control logic on an I/O primitive. This operation may be used
/// to execute any custom logic that is not a match for the other elementary I/O operations.
///
/// The primary distinguishing factor of this type of operation is that it does not use any I/O
/// subsystem owned memory - neither as input or output. If data input or output is required, this
/// must be accomplished through memory owned by the caller directly.
///
/// User code typically does not need to interact with this type directly, as it is primarily used
/// to build higher-level I/O endpoint types (e.g. file, socket, tcp server, ...).
#[derive(Debug)]
pub struct ControlOperation {
    // This ensures that the primitive we are operating on is kept alive until we issue the I/O
    // to the operating system (assuming that it is even alive once we start the operation - there
    // may be a delay between the user enqueuing I/O and the operation actually starting).
    primitive: Weak<BoundPrimitiveRef>,

    offset: u64,
    user_resources: Option<Box<dyn UserResource>>,
    resources: Arc<Resources>,
}

impl ControlOperation {
    pub(crate) fn new(primitive: Weak<BoundPrimitiveRef>, resources: Arc<Resources>) -> Self {
        Self {
            primitive,
            offset: 0,
            user_resources: None,
            resources,
        }
    }

    /// For seekable I/O primitives (e.g. files), sets the offset in the file where the operation
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
    /// The callback may be called at any point in the future - it is not guaranteed to be called
    /// during the call to this method. If the primitive is closed before the callback is started,
    /// the operation is canceled.
    ///
    /// # Panics
    ///
    /// Panics if the callback never requested the platform-specific system call parameters from
    /// the `ControlOperationArgs` struct, indicating that it did not perform any system call.
    pub async fn begin<C>(mut self, callback: C) -> Result<(), crate::Error>
    where
        C: FnOnce(&BoundPrimitiveRef, ControlOperationArgs) -> BeginResult<()> + Send + 'static,
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

        let mut operation = Operation::new(
            self.offset,
            MemoryGuard::default(),
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

        let args = ControlOperationArgs {
            elementary_operation_key: Some(elementary_operation_key),
        };

        // Note: during the callback and (if it completes asynchronously) until a completion
        // is received, the operating system owns the elementary operation (and by
        // extension the entire Operation instance, simply due to Rust not knowing the
        // difference). We should not dereference the operation during this period.
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
                    key = elementary_operation_key.0
                );

                return result;
            }
        }

        // We register the operation in the resource set so that the I/O driver
        // can find it once we receive the completion notification.
        self.resources
            .operations_mut()
            .insert(elementary_operation_key, operation);

        _ = result_rx
            .await
            .expect("I/O driver side of an I/O operation vanished while awaiting result")?;

        Ok(())
    }
}

/// Arguments provided by the I/O subsystem to the `begin()` callback that executes an elementary
/// I/O operation to send a control message to an I/O primitive.
#[derive(Debug)]
pub struct ControlOperationArgs {
    // This is Option because it can only be called once (to consume it) because each `begin()` is
    // only intended to start a single system call. We use the Option to protect against double-get
    // and to ensure (via Drop check) that the caller does consume it (if not - no syscall made?!)
    //
    // Of course, even if the callback asks for the system call parameters exactly once, there is
    // no guarantee we can get here that it will use the parameters to actually make a system call
    // but if it does not, that is merely a memory leak and not a safety issue, so good enough.
    elementary_operation_key: Option<ElementaryOperationKey>,
}

impl ControlOperationArgs {
    /// # Panics
    ///
    /// Panics if called more than once.
    pub(crate) const fn consume_elementary_operation_key(&mut self) -> ElementaryOperationKey {
        self.elementary_operation_key
            .take()
            .expect("elementary operation consumed more than once from ControlOperationArgs")
    }
}

impl Drop for ControlOperationArgs {
    fn drop(&mut self) {
        assert!(
            self.elementary_operation_key.is_none(),
            "system call parameters not consumed from ControlOperationArgs - no system call could have been made"
        );
    }
}

#[negative_impl]
impl !Send for ControlOperationArgs {}
#[negative_impl]
impl !Sync for ControlOperationArgs {}

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
    use crate::ERR_POISONED_LOCK;
    use crate::pal::{MockPlatform, PlatformFacade};
    use crate::testing::{
        bind_dummy_primitive, expect_elementary_operations, new_failed_completion_notification,
        new_successful_completion_notification, use_default_memory_pool,
        use_simulated_completion_queue, with_partial_io_test_harness_and_platform,
    };

    #[test]
    fn thread_mobile_type() {
        assert_impl_all!(ControlOperation: Send);
    }

    #[test]
    fn args_is_single_threaded_type() {
        assert_not_impl_any!(ControlOperationArgs: Send, Sync);
    }

    #[test]
    fn control_success_async() {
        let mut platform = MockPlatform::new();

        use_default_memory_pool::<1234>(&mut platform);
        expect_elementary_operations(1, &mut platform);

        let queue_simulation = use_simulated_completion_queue(&mut platform);
        let platform = PlatformFacade::from_mock(platform);

        with_partial_io_test_harness_and_platform(platform, async move |test_harness| {
            let primitive = bind_dummy_primitive(&test_harness.context);

            let future = primitive.control().begin(move |_primitive, mut args| {
                queue_simulation.completed.lock().unwrap().push_back(
                    new_successful_completion_notification(
                        args.consume_elementary_operation_key(),
                        0,
                    ),
                );

                BeginResult::Asynchronous
            });
            let mut future = pin!(future);

            let mut cx = Context::from_waker(futures::task::noop_waker_ref());
            let poll_result = future.as_mut().poll(&mut cx);
            assert!(matches!(poll_result, Poll::Pending));

            // We expect this to process the completion which is already in the queue.
            test_harness.driver.borrow_mut().process_completions(0);
            let mut cx = Context::from_waker(futures::task::noop_waker_ref());
            let poll_result = future.as_mut().poll(&mut cx);

            match poll_result {
                Poll::Ready(Ok(())) => {}
                _ => panic!("unexpected poll result: {poll_result:?}"),
            }
        });
    }

    #[test]
    fn control_success_sync() {
        let mut platform = MockPlatform::new();

        use_default_memory_pool::<1234>(&mut platform);
        expect_elementary_operations(1, &mut platform);

        use_simulated_completion_queue(&mut platform);

        let platform = PlatformFacade::from_mock(platform);

        with_partial_io_test_harness_and_platform(platform, async move |test_harness| {
            let primitive = bind_dummy_primitive(&test_harness.context);

            let future = primitive.control().begin(move |_primitive, mut args| {
                // We must consume the elementary operation key to indicate that we have made
                // (or simulated) a system call, because this is mandatory in real code.
                args.consume_elementary_operation_key();

                BeginResult::CompletedSynchronously(Ok(()))
            });
            let mut future = pin!(future);

            let mut cx = Context::from_waker(futures::task::noop_waker_ref());
            let poll_result = future.as_mut().poll(&mut cx);

            match poll_result {
                Poll::Ready(Ok(())) => {}
                _ => panic!("unexpected poll result: {poll_result:?}"),
            }
        });
    }

    #[test]
    fn control_failed_async() {
        let mut platform = MockPlatform::new();

        use_default_memory_pool::<1234>(&mut platform);
        expect_elementary_operations(1, &mut platform);

        let queue_simulation = use_simulated_completion_queue(&mut platform);

        let platform = PlatformFacade::from_mock(platform);

        with_partial_io_test_harness_and_platform(platform, async move |test_harness| {
            let primitive = bind_dummy_primitive(&test_harness.context);

            let future = primitive.control().begin(move |_primitive, mut args| {
                queue_simulation.completed.lock().unwrap().push_back(
                    new_failed_completion_notification(args.consume_elementary_operation_key()),
                );

                BeginResult::Asynchronous
            });

            let mut future = pin!(future);

            let mut cx = Context::from_waker(futures::task::noop_waker_ref());
            let poll_result = future.as_mut().poll(&mut cx);
            assert!(matches!(poll_result, Poll::Pending));

            // We expect this to process the completion which is already in the queue.
            test_harness.driver.borrow_mut().process_completions(0);

            let mut cx = Context::from_waker(futures::task::noop_waker_ref());
            let poll_result = future.as_mut().poll(&mut cx);
            // We do not have an API contract for what error it must be.
            assert!(matches!(poll_result, Poll::Ready(Err(_))));
        });
    }

    #[test]
    fn control_failed_sync() {
        let mut platform = MockPlatform::new();

        use_default_memory_pool::<1234>(&mut platform);
        expect_elementary_operations(1, &mut platform);

        use_simulated_completion_queue(&mut platform);

        let platform = PlatformFacade::from_mock(platform);

        with_partial_io_test_harness_and_platform(platform, async move |test_harness| {
            let primitive = bind_dummy_primitive(&test_harness.context);

            let future = primitive.control().begin(move |_primitive, mut args| {
                // We must consume the elementary operation key to indicate that we have made
                // (or simulated) a system call, because this is mandatory in real code.
                args.consume_elementary_operation_key();

                BeginResult::CompletedSynchronously(Err(std::io::Error::new(
                    ErrorKind::AlreadyExists,
                    "something went wrong".to_string(),
                )
                .into()))
            });
            let mut future = pin!(future);

            let mut cx = Context::from_waker(futures::task::noop_waker_ref());
            let poll_result = future.as_mut().poll(&mut cx);
            // We do not have an API contract for what error it must be.
            assert!(matches!(poll_result, Poll::Ready(Err(_))));
        });
    }

    #[test]
    fn control_offset_reached_platform() {
        let mut platform = MockPlatform::new();

        use_default_memory_pool::<1234>(&mut platform);
        let operation_offsets = expect_elementary_operations(1, &mut platform);

        let queue_simulation = use_simulated_completion_queue(&mut platform);

        let platform = PlatformFacade::from_mock(platform);

        with_partial_io_test_harness_and_platform(platform, async move |test_harness| {
            let primitive = bind_dummy_primitive(&test_harness.context);

            let future =
                primitive
                    .control()
                    .with_offset(625_453)
                    .begin(move |_primitive, mut args| {
                        let elementary_operation_key = args.consume_elementary_operation_key();

                        queue_simulation.completed.lock().unwrap().push_back(
                            new_successful_completion_notification(elementary_operation_key, 0),
                        );

                        // The contract of expect_elementary_operations() says the key is the index
                        // into the operation_offsets list.
                        assert_eq!(
                            operation_offsets.lock().expect(ERR_POISONED_LOCK)
                                [elementary_operation_key.0],
                            Some(625_453)
                        );

                        BeginResult::Asynchronous
                    });
            let mut future = pin!(future);

            let mut cx = Context::from_waker(futures::task::noop_waker_ref());
            let poll_result = future.as_mut().poll(&mut cx);
            assert!(matches!(poll_result, Poll::Pending));

            // We expect this to process the completion which is already in the queue.
            test_harness.driver.borrow_mut().process_completions(0);

            let mut cx = Context::from_waker(futures::task::noop_waker_ref());
            let poll_result = future.as_mut().poll(&mut cx);

            match poll_result {
                Poll::Ready(Ok(())) => {}
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

                let future = primitive.control().with_resources(resource).begin(
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
                let mut future = pin!(future);

                let mut cx = Context::from_waker(futures::task::noop_waker_ref());
                let poll_result = future.as_mut().poll(&mut cx);
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
    fn control_no_syscall_panic() {
        let mut platform = MockPlatform::new();

        use_default_memory_pool::<1234>(&mut platform);
        expect_elementary_operations(1, &mut platform);

        use_simulated_completion_queue(&mut platform);

        let platform = PlatformFacade::from_mock(platform);

        with_partial_io_test_harness_and_platform(platform, |test_harness| async move {
            let primitive = bind_dummy_primitive(&test_harness.context);

            let future = primitive
                .control()
                // We must consume the elementary operation key, even if we don't use it, to
                // indicate that we have made (or simulated) a system call. We do not!
                .begin(|_, _| BeginResult::CompletedSynchronously(Ok(())));
            let mut future = pin!(future);

            let mut cx = Context::from_waker(futures::task::noop_waker_ref());
            let poll_result = future.as_mut().poll(&mut cx);

            match poll_result {
                Poll::Ready(Ok(())) => {}
                _ => panic!("unexpected poll result: {poll_result:?}"),
            }
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

            let future = primitive.control().begin(move |_primitive, mut args| {
                // Calling this twice is invalid, as it implies we are making two syscalls
                // in a single callback, which is not allowed. The second call should panic.
                args.consume_elementary_operation_key();
                args.consume_elementary_operation_key();

                BeginResult::CompletedSynchronously(Ok(()))
            });
            let mut future = pin!(future);

            let mut cx = Context::from_waker(futures::task::noop_waker_ref());
            let poll_result = future.as_mut().poll(&mut cx);

            match poll_result {
                Poll::Ready(Ok(())) => {}
                _ => panic!("unexpected poll result: {poll_result:?}"),
            }
        });
    }
}