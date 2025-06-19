// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::Arc;
use std::thread;

use crate::pal::{CompletionNotification, CompletionQueue, PlatformFacade};
use crate::{Context, Resources, Runtime, Waker};

/// The I/O driver is used by an async task runtime to facilitate the execution of I/O operations
/// initiated by I/O endpoints built on the Oxidizer I/O subsystem.
///
/// Each I/O context is associated with an I/O driver. The I/O driver is responsible for
/// coordinating I/O operations with the operating system and receiving notifications of I/O
/// completion.
///
/// # What does it do?
///
/// The primary function of an I/O driver is to make `.await` calls on I/O operations complete when
/// the operating system tells it that an operation has finished.
///
/// I/O endpoints and application code do not access the I/O driver. It is designed to be used
/// by the async task runtime that owns the threads of the current process. All "outward facing"
/// I/O operations are initiated via the [I/O context][1], references to which can be obtained from
/// the I/O driver via [`.context()`][2].
///
/// # Integration
///
/// The async task runtime must call [`process_completions()`][5] whenever there is no non-I/O work
/// to do. This call will return when at least one I/O operation has completed (which potentially
/// creates non-I/O work for the application). The async task runtime should pass a long timeout
/// argument to this call, so when there is nothing to do, every thread will be waiting on
/// [`process_completions()`][5].
///
/// When the async task runtime wants to exit [`process_completions()`][5] early (e.g. because
/// another thread enqueued some non-I/O work), it can use the [`Waker`][6] returned by
/// [`waker()`][7] to wake up the thread that is blocked in [`process_completions()`][5].
///
/// # Safety
///
/// The I/O driver must not be dropped while any I/O operation is in progress or while any
/// bound I/O primitive remains alive. To shut down safely, the I/O driver must be kept
/// active until it signals that all I/O operations have completed and I/O resources
/// (such as [bound primitives][3]) have been released, by waiting until [`is_inert()`][4]
/// returns true. While the I/O driver remains active, [`process_completions()`][5] must be called
/// regularly.
///
/// Note that the I/O driver will not proactively release resources - it is the
/// responsibility of the driver's owner (ultimately, the application) to close all I/O primitives
/// to facilitate resource release and graceful shutdown. For example, the application may want to
/// either continue executing to completion or simply drop tasks that may be holding I/O resources.
///
/// # Ownership
///
/// The I/O driver is exclusively owned by the async task runtime that uses it to process I/O
/// events.
///
/// # Thread safety
///
/// While thread-mobile, an I/O driver can only be used concurrently from one thread (i.e. guarded
/// by `Mutex` or similar). To efficiently and concurrently process I/O events at scale, multiple
/// I/O drivers must be used, typically one per thread.
///
/// [1]: crate::Context
/// [2]: crate::Driver::context
/// [3]: crate::BoundPrimitive
/// [4]: crate::Driver::is_inert
/// [5]: crate::Driver::process_completions
/// [6]: crate::Waker
/// [7]: Self::waker
#[derive(Debug)]
pub struct Driver {
    resources: Arc<Resources>,
    context: Arc<Context>,
}

impl Driver {
    /// Creates a new I/O driver capable of servicing I/O on the current thread.
    ///
    /// # Safety
    ///
    /// The I/O driver must not be dropped while any I/O operation is in progress or while any
    /// bound I/O primitive remains alive. To shut down safely, the I/O driver must be kept
    /// active until it signals that all I/O operations have completed and I/O resources
    /// (such as [bound primitives][3]) have been released, by waiting until [`is_inert()`][4]
    /// returns true. While the I/O driver remains active, [`process_completions()`][5] must be called
    /// regularly.
    ///
    /// Note that the I/O driver will not proactively release resources - it is the
    /// responsibility of the driver's owner (ultimately, the application) to close all I/O primitives
    /// to facilitate resource release and graceful shutdown. For example, the application may want to
    /// either continue executing to completion or simply drop tasks that may be holding I/O resources.
    ///
    /// [3]: crate::BoundPrimitive
    /// [4]: crate::Driver::is_inert
    /// [5]: crate::Driver::process_completions
    #[must_use]
    pub unsafe fn new(rt: Box<dyn Runtime>) -> Self {
        // SAFETY: Forwarding safety requirements to the caller.
        unsafe { Self::with_runtime_and_platform(rt, PlatformFacade::real()) }
    }

    /// # Safety
    ///
    /// The I/O driver must not be dropped while any I/O operation is in progress or while any
    /// bound I/O primitive remains alive. To shut down safely, the I/O driver must be kept
    /// active until it signals that all I/O operations have completed and I/O resources
    /// (such as [bound primitives][3]) have been released, by waiting until [`is_inert()`][4]
    /// returns true. While the I/O driver remains active, [`process_completions()`][5] must be called
    /// regularly.
    ///
    /// Note that the I/O driver will not proactively release resources - it is the
    /// responsibility of the driver's owner (ultimately, the application) to close all I/O primitives
    /// to facilitate resource release and graceful shutdown. For example, the application may want to
    /// either continue executing to completion or simply drop tasks that may be holding I/O resources.
    ///
    /// [3]: crate::BoundPrimitive
    /// [4]: crate::Driver::is_inert
    /// [5]: crate::Driver::process_completions
    pub(crate) unsafe fn with_runtime_and_platform(
        rt: Box<dyn Runtime>,
        pal: PlatformFacade,
    ) -> Self {
        let rt = Arc::new(rt);
        let resources = Arc::new(Resources::new(rt, pal));

        Self {
            resources: Arc::clone(&resources),
            context: Context::new(resources).into(),
        }
    }

    /// Processes completed I/O operations. This causes `await` statements waiting on I/O operations
    /// to resume execution.
    ///
    /// If no I/O operations have completed when this is called, waits up to `max_wait_time_millis`
    /// milliseconds for at least one operation to complete before returning.
    ///
    /// Use a [`Waker`][1] to wake up a thread that is blocked in this function. When awakened,
    /// an ongoing call into this function will behave as if it had been issued with a timeout of
    /// zero. You can obtain a waker by calling [`waker()`][2].
    ///
    /// [1]: crate::Waker
    /// [2]: Self::waker
    ///
    /// # Panics
    ///
    /// TODO: Document panics
    pub fn process_completions(&self, max_wait_time_millis: u32) {
        let mut completion_queue = self.resources.completion_queue_mut();

        completion_queue.process_completions(max_wait_time_millis, |entry| {
            let mut operation = self
                .resources
                .operations_mut()
                .remove(&entry.elementary_operation_key())
                .expect(
                    "completed elementary I/O operation was not in the set of active operations; this can happen if the *Operation::begin() callback incorrectly signaled synchronous completion when the operation was in fact enqueued asynchronously; this can also happen if the I/O primitive was bound to a different I/O driver than the one used to start the operation",
                );

            let result_tx = operation.take_result_tx();

            // We ignore the send() return value because we do not care if the receiver has been
            // dropped and nothing receives the result - our job here is done either way.
            _ = result_tx.send(entry.result());
        });
    }

    /// Whether the driver has entered a state where it is safe to drop it. See safety requirements
    /// of [`new()`][1] for more details.
    ///
    /// [1]: crate::Driver::new
    #[must_use]
    pub fn is_inert(&self) -> bool {
        // We wait for all I/O operations to complete, as we need to ensure that any resources we
        // shared with the operating system remain alive for the duration of the operations.
        self.resources.operations_mut().is_empty()
            // We wait for all bound primitives to be released, as we need to ensure we do not leak
            // any resources. If a primitive tries to start cleanup after I/O driver shutdown, it
            // may fail, as the I/O driver shutdown could be taken as a signal by higher layers to
            // terminate the application/runtime. By default, there is no "wait for completion".
            && self.resources.primitives().is_empty()
    }

    /// Returns the I/O context associated with this I/O driver. The I/O context is used by I/O
    /// endpoint implementations (file, socket, ...) to interact with the I/O driver.
    #[must_use]
    #[expect(
        clippy::missing_const_for_fn,
        reason = "Analysis is wrong, this can't be made const based on current Rust rules"
    )]
    pub fn context(&self) -> &Context {
        &self.context
    }

    /// Creates a new waker that can be used to wake up the current thread when it is blocked
    /// waiting for an I/O completion in [`process_completions()`][Self::process_completions].
    #[must_use]
    pub fn waker(&self) -> Waker {
        Waker::new(self.resources.completion_queue().waker())
    }
}

impl Drop for Driver {
    fn drop(&mut self) {
        if thread::panicking() {
            // We skip the assertion if we are already panicking because a double panic more often
            // does not help anything and may even obscure the initial panic in test runs.
            return;
        }

        // We must ensure that all I/O resources are released before we drop the driver. This is
        // a safety requirement of the driver - if it is not inert, we are violating memory safety.
        assert!(self.is_inert(), "I/O driver dropped without safe shutdown");
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic;

    use futures::executor::LocalPool;

    use super::*;
    use crate::pal::MockPlatform;
    #[cfg(windows)]
    use crate::testing::{IoPumpMode, with_io_test_harness_ex};
    use crate::testing::{
        TestRuntime, bind_dummy_primitive, expect_elementary_operations, use_default_memory_pool,
        use_simulated_completion_queue, with_partial_io_test_harness_and_platform,
    };

    #[test]
    #[should_panic(expected = "I/O driver dropped without safe shutdown")]
    fn drop_when_in_use() {
        let mut platform = MockPlatform::new();

        use_default_memory_pool::<1234>(&mut platform);
        expect_elementary_operations(1, &mut platform);

        _ = use_simulated_completion_queue(&mut platform);

        let platform = PlatformFacade::from_mock(platform);

        let executor = LocalPool::new();
        let runtime = TestRuntime::new(&executor.spawner());

        // SAFETY: We are intentionally violating safety requirements here.
        let driver =
            unsafe { Driver::with_runtime_and_platform(Box::new(runtime.client()), platform) };
        let context = driver.context();

        // Registering the primitive requires unregistration before drop, which we do not do.
        let bound_primitive = bind_dummy_primitive(context);

        // And we just drop it here! This is a safety violation, as the I/O must not be dropped
        // without a graceful shutdown. We must be careful here, though! Dropping the driver
        // without correct resource management may also cause *other* panics, which we do NOT
        // want to accept. Therefore, we check for the specific panic message.
        drop(driver);

        // This ensures that any unregistration cannot happen before driver drop,
        // by ensuring we keep the primitive alive until this point.
        drop(bound_primitive);
    }

    #[test]
    fn waker_signals_completion_queue() {
        let mut platform = MockPlatform::new();

        use_default_memory_pool::<1234>(&mut platform);
        expect_elementary_operations(0, &mut platform);

        let queue_simulation = use_simulated_completion_queue(&mut platform);
        let platform = PlatformFacade::from_mock(platform);

        with_partial_io_test_harness_and_platform(platform, |test_harness| async move {
            // Super trivial test - all we care about is that if the public API waker is
            // told to wake the I/O driver, the wake signal goes to the completion queue.
            let driver = test_harness.driver.borrow();
            let waker = driver.waker();

            assert_eq!(
                queue_simulation
                    .wake_signals_received
                    .load(atomic::Ordering::Relaxed),
                0
            );

            waker.wake();

            assert_eq!(
                queue_simulation
                    .wake_signals_received
                    .load(atomic::Ordering::Relaxed),
                1
            );
        });
    }

    #[cfg(not(miri))] // Miri cannot do real I/O.
    #[cfg(windows)] // Real I/O only works on Windows for now.
    #[test]
    fn real_waker_really_wakes() {
        with_io_test_harness_ex(
            None,
            // We do our own I/O event pumping in this one.
            IoPumpMode::ShutdownOnly,
            |harness| async move {
                let driver = harness.driver.borrow();
                let waker = driver.waker();

                // Paranoia - if something in a future iteration of the test harness queues
                // up events at the start of test, this should drain them.
                driver.process_completions(0);

                // Throw it over the fence to another thread, just for extra realism.
                thread::spawn(move || {
                    // We immediately signal a wake. Because there is no I/O happening, we
                    // should immediately return from the first I/O poll now, with zero events.
                    // Wake-up signals are enqueued for the next "block on IO" operation,
                    // so it is fine to do this ahead of time.
                    waker.wake();
                })
                .join()
                .unwrap();

                // If the wake-up failed, the test will timeout here shortly. We set a 5 minute
                // poll timeout but the test harness times out much faster, of course.
                driver.process_completions(300_000_000);
            },
        );
    }

    #[cfg(not(miri))] // Miri cannot do real I/O.
    #[test]
    fn create_driver_basic_operations_ok() {
        let executor = LocalPool::new();
        let runtime = TestRuntime::new(&executor.spawner());

        // SAFETY: Just test code.
        let driver = unsafe { Driver::new(Box::new(runtime.client())) };

        // Check that minimal functionality does not cause panic, this allows us to have IO driver
        // exposed as a type even on Linux.
        let _context = driver.context();
        driver.process_completions(10);
        driver.waker().wake();
    }
}