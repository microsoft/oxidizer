// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use futures::executor::{LocalPool, LocalSpawner};
use oxidizer_testing::{TEST_TIMEOUT, execute_or_terminate_process};

use crate::testing::{IoPumpEntrypoint, IoPumpMode, TestRuntime, TestRuntimeStats};
use crate::{Context, Driver, Runtime};

/// Executes an asynchronous function with an I/O subsystem test harness, setting up a test runtime,
/// running the function, and tearing down the runtime.
///
/// Automatically applies a timeout to the I/O subsystem shutdown process and to the function body,
/// as tests are expected to complete fast.
pub fn with_io_test_harness<FF, F, R>(f: FF) -> R
where
    FF: FnOnce(Context) -> F,
    F: Future<Output = R>,
{
    with_io_test_harness_ex(None, IoPumpMode::Always, move |harness| f(harness.context))
}

/// Exposes the internals of the test harness for inspection and manipulation during a unit test.
#[derive(Debug)]
pub struct TestHarnessEx {
    pub driver: Rc<RefCell<Driver>>,
    pub context: Context,
    pub runtime_stats: Arc<TestRuntimeStats>,
    pub spawner: LocalSpawner,
}

/// Executes an asynchronous function with an I/O subsystem test harness, setting up a test runtime,
/// running the function, and tearing down the runtime.
///
/// Uses a custom callback to create the I/O
/// driver. Allows the IO pump mode to be customized and grants direct access to the I/O driver and
/// test runtime metrics.
///
/// The test harness guarantees that the I/O driver is not dropped until it signal it is inert.
///
/// Automatically applies a timeout to the I/O subsystem shutdown process and to the function body,
/// as tests are expected to complete fast.
#[expect(
    clippy::type_complexity,
    reason = "Will allow it since it is used only once"
)]
pub fn with_io_test_harness_ex<FF, F, R>(
    // None means just use real driver.
    driver_provider: Option<Box<dyn FnOnce(Box<dyn Runtime>) -> Driver>>,
    io_pump_mode: IoPumpMode,
    f: FF,
) -> R
where
    FF: FnOnce(TestHarnessEx) -> F,
    F: Future<Output = R>,
{
    let driver_provider = driver_provider.unwrap_or_else(|| Box::new(real_driver_provider));

    let mut executor = LocalPool::new();
    let runtime = TestRuntime::new(&executor.spawner());
    let runtime_stats = runtime.stats();

    let driver = Rc::new(RefCell::new(driver_provider(Box::new(runtime.client()))));
    let harness = TestHarnessEx {
        driver: Rc::clone(&driver),
        context: driver.borrow().context().clone(),
        runtime_stats,
        spawner: executor.spawner(),
    };

    // The entrypoint wrapper ensures that we fulfill the safety promise of the I/O driver,
    // to not exit until all I/O operations are complete.
    let io_pump = IoPumpEntrypoint::new(
        Rc::clone(&driver),
        io_pump_mode,
        Some(TEST_TIMEOUT),
        Some(TEST_TIMEOUT),
        f(harness),
    );

    // Timeout guard in case there is a synchronous deadlock somewhere in the works.
    execute_or_terminate_process(move || executor.run_until(io_pump))
}

fn real_driver_provider(rt: Box<dyn Runtime>) -> Driver {
    // SAFETY: We are required not to drop this instance until .is_inert() returns true.
    // This is guaranteed by IoPumpEntrypoint which we apply above.
    unsafe { Driver::new(rt) }
}