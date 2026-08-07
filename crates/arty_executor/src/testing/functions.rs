// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::task::Waker;
use std::time::Duration;
use std::{env, thread};

use scopeguard::{Always, ScopeGuard};

use crate::{CycleOutcome, Executor};

/// We do not want the timeout panic to occur under mutation testing because that makes for slow
/// mutation tests. Instead, we want the mutation test harness itself to time out! Therefore, this
/// is a very high value to ensure that under mutation testing the executor timeout logic will
/// never trigger.
const MUTATION_TESTING_SHUTDOWN_TIMEOUT: Duration = Duration::from_mins(15);
const TEST_TIMEOUT: Duration = Duration::from_secs(10);

#[cfg_attr(test, mutants::skip)] // Test-harness configuration is not production behavior.
fn is_mutation_testing() -> bool {
    env::var("MUTATION_TESTING").as_deref() == Ok("1")
}

/// Creates a new `Executor` with a scopeguard that ensures safe shutdown on drop.
///
/// # Panics
///
/// The scopeguard drop logic panics when shutdown times out (except if mutation testing).
#[must_use]
#[cfg_attr(test, mutants::skip)] // This is test logic, mutation here is unhelpful.
pub fn new_guarded_executor(owner_waker: Waker) -> ScopeGuard<Executor, fn(Executor), Always> {
    scopeguard::guard(
        {
            let mut builder = Executor::builder().owner_waker(owner_waker);

            if is_mutation_testing() {
                builder = builder.shutdown_timeout(MUTATION_TESTING_SHUTDOWN_TIMEOUT);
            } else {
                builder = builder.shutdown_timeout(TEST_TIMEOUT);
            }

            // SAFETY: We are not allowed to drop it without the proper shutdown process.
            // That is the whole point of this guard, so we are all good on that front.
            unsafe { builder.build() }
        },
        |executor: Executor| {
            executor.begin_shutdown();

            while executor.execute_cycle() != CycleOutcome::Shutdown {
                // There is nothing else for us to do but to keep going.
                // `execute_cycle()` will take care of triggering timeout.
                thread::yield_now();
            }
        },
    )
}
