// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::collections::VecDeque;
use std::io::ErrorKind;
use std::num::NonZeroUsize;
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex};

use crate::pal::{
    DefaultMemoryPool, ElementaryOperationKey, MockCompletionNotification, MockElementaryOperation,
    MockPlatform, MockPrimitive, PlatformFacade,
};
use crate::testing::{
    CompletionNotificationSource, IoPumpMode, SimulatedCompletionQueue, TestHarnessEx,
    with_io_test_harness_ex,
};
use crate::{BoundPrimitive, Context, Driver, ERR_POISONED_LOCK, UnboundPrimitive};

/// Executes an asynchronous function with an I/O subsystem test harness, setting up a test runtime,
/// running the function, and tearing down the runtime. Uses the provided PAL facade when setting
/// up the I/O subsystem.
///
/// Requires the caller to manually process I/O events during callback execution. During shutdown,
/// after the callback returns, I/O events are still automatically processed.
///
/// Automatically applies a timeout to the I/O subsystem shutdown process and to the function body,
/// as tests are expected to complete fast.
#[cfg_attr(test, mutants::skip)] // This is test code, we do not care about mutating it.
pub fn with_partial_io_test_harness_and_platform<FF, F, R>(platform: PlatformFacade, f: FF) -> R
where
    FF: FnOnce(TestHarnessEx) -> F,
    F: Future<Output = R>,
{
    with_io_test_harness_ex(
        Some(Box::new(|rt| {
            // SAFETY: We are required not to drop this instance until .is_inert() returns true.
            // This is guaranteed by the test harness.
            unsafe { Driver::with_runtime_and_platform(rt, platform) }
        })),
        IoPumpMode::ShutdownOnly,
        f,
    )
}

/// Configures a mock platform to perform real memory allocations using the default memory pool.
#[cfg_attr(test, mutants::skip)] // This is test code, we do not care about mutating it.
pub fn use_default_memory_pool<const BLOCK_SIZE: usize>(pal: &mut MockPlatform) {
    let block_size = NonZeroUsize::new(BLOCK_SIZE).unwrap();

    pal.expect_new_memory_pool()
        .returning(move || DefaultMemoryPool::new(block_size).into());
}

#[cfg_attr(test, mutants::skip)] // This is test code, we do not care about mutating it.
pub fn new_successful_completion_notification(
    elementary_operation_key: ElementaryOperationKey,
    bytes_transferred: u32,
) -> MockCompletionNotification {
    let mut mock = MockCompletionNotification::new();
    mock.expect_elementary_operation_key()
        .return_const(elementary_operation_key);
    mock.expect_result()
        .returning(move || Ok(bytes_transferred));

    mock
}

#[cfg_attr(test, mutants::skip)] // This is test code, we do not care about mutating it.
pub fn new_failed_completion_notification(
    elementary_operation_key: ElementaryOperationKey,
) -> MockCompletionNotification {
    let mut mock = MockCompletionNotification::new();
    mock.expect_elementary_operation_key()
        .return_const(elementary_operation_key);
    mock.expect_result().returning(move || {
        Err(
            std::io::Error::new(ErrorKind::AlreadyExists, "something went wrong".to_string())
                .into(),
        )
    });

    mock
}

/// Creates a mock primitive that does nothing useful but functions correctly.
#[cfg_attr(test, mutants::skip)] // This is test code, we do not care about mutating it.
pub fn new_dummy_primitive() -> MockPrimitive {
    let mut mock = MockPrimitive::new();

    // It must be possible to clone it. Clones are just new dummy primitives.
    mock.expect_clone().returning(new_dummy_primitive);

    // It can be closed. This is just a no-op. Not every primitive gets closed (only one clone),
    // so we cannot expect a count here (use a specialized mock for that)
    mock.expect_close().return_const(());

    mock
}

/// Binds a dummy primitive to a context, just to verify that primitive management logic
/// is exercised in a test (even though the mock platform generally does not require any
/// primitives to be bound).
#[cfg_attr(test, mutants::skip)] // This is test code, we do not care about mutating it.
pub fn bind_dummy_primitive(context: &Context) -> BoundPrimitive {
    let original_primitive = new_dummy_primitive();
    let unbound_primitive = UnboundPrimitive::from_mock(original_primitive);

    context.bind_primitive(unbound_primitive).unwrap()
}

/// Uses a simulated completion queue that returns at most one completion per poll from the
/// returned queue, and never waits for more completions.
#[cfg_attr(test, mutants::skip)] // This is test code, we do not care about mutating it.
pub fn use_simulated_completion_queue(pal: &mut MockPlatform) -> CompletionQueueSimulationState {
    let completions_source = Arc::new(Mutex::new(VecDeque::new()));
    let wake_signals_received = Arc::new(AtomicUsize::new(0));

    pal.expect_new_completion_queue().times(1).returning({
        let completions_source = Arc::clone(&completions_source);
        let wake_signals_received = Arc::clone(&wake_signals_received);
        move || {
            SimulatedCompletionQueue::new(
                Arc::clone(&completions_source),
                Arc::clone(&wake_signals_received),
            )
            .into()
        }
    });

    CompletionQueueSimulationState {
        completed: completions_source,
        wake_signals_received,
    }
}

#[derive(Debug)]
pub struct CompletionQueueSimulationState {
    pub completed: CompletionNotificationSource,
    pub wake_signals_received: Arc<AtomicUsize>,
}

/// Configures the platform to allow/expect a specific number of elementary I/O operations.
///
/// Returns a vector with cells that will be filled with the offsets at which each elementary
/// operation is started, once started. The elementary operation key will be the index into
/// this list.
#[cfg_attr(test, mutants::skip)] // This is test code, we do not care about mutating it.
pub fn expect_elementary_operations(
    count: usize,
    pal: &mut MockPlatform,
) -> Arc<Mutex<Vec<Option<u64>>>> {
    let offsets = Arc::new(Mutex::new(vec![None; count]));
    let mut next_index: usize = 0;

    pal.expect_new_elementary_operation()
        .times(count)
        .returning({
            let offsets = Arc::clone(&offsets);

            move |offset| {
                let mut mock = MockElementaryOperation::new();

                mock.expect_offset().return_const(offset);

                // For mock elementary operations, the key is just the index into the sequence of
                // elementary operations whose creation we are expecting here.
                mock.expect_key()
                    .return_const(ElementaryOperationKey(next_index));

                *offsets
                    .lock()
                    .expect(ERR_POISONED_LOCK)
                    .get_mut(next_index)
                    .expect("more elementary operations were created than we were expecting") =
                    Some(offset);

                next_index = next_index
                    .checked_add(1)
                    .expect("overflow is inconceivable here");

                mock.into()
            }
        });

    offsets
}

// We assert unwind safety here because the topic is too much hassle to worry about and since
// #[should_panic] does not require us to worry about it, we are not going to worry about it here.
macro_rules! assert_panic {
    ($stmt:stmt$(,)?) => {
        #[allow(clippy::multi_assignments, reason = "macro untidyness")]
        #[expect(clippy::allow_attributes, reason = "macro untidyness")]
        ::std::panic::catch_unwind(::std::panic::AssertUnwindSafe(|| -> () { _ = { $stmt } }))
            .expect_err("assert_panic! argument did not panic")
    };
    ($stmt:stmt, $expected:expr$(,)?) => {
        #[allow(clippy::multi_assignments, reason = "macro untidyness")]
        #[expect(clippy::allow_attributes, reason = "macro untidyness")]
        match ::std::panic::catch_unwind(::std::panic::AssertUnwindSafe(|| -> () { _ = { $stmt } }))
        {
            Ok(_) => panic!("assert_panic! argument did not panic"),
            Err(err) => {
                let panic_msg = err
                    .downcast_ref::<String>()
                    .map(|s| s.asr())
                    .or_else(|| err.downcast_ref::<&str>().copied())
                    .expect("panic message must be a string");
                assert_eq!(
                    panic_msg, $expected,
                    "expected panic message '{}', but got '{}'",
                    $expected, panic_msg
                );
            }
        }
    };
}

/// Wraps a block of code with `futures::executor::block_on` and test timeout logic.
macro_rules! async_test {
    ($($code:tt)*) => {
        ::oxidizer_testing::execute_or_terminate_process(|| {
            ::futures::executor::block_on(async {
                $($code)*
            });
        });
    };
}

pub(crate) use {assert_panic, async_test};