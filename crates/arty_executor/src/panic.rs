// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::Any;
use std::panic::{AssertUnwindSafe, catch_unwind};

/// A panic captured while executing or disposing an executor task.
pub type TaskPanicPayload = Box<dyn Any + Send + 'static>;

/// Disposes a captured task panic without allowing a panicking payload destructor to unwind.
#[doc(hidden)]
#[cfg_attr(coverage_nightly, coverage(off))] // Terminal repeated-panic termination is child-process-only.
pub fn discard_task_panic_payload(mut panic: TaskPanicPayload) {
    const MAX_DISPOSAL_ATTEMPTS: usize = 32;
    for _ in 0..MAX_DISPOSAL_ATTEMPTS {
        match catch_unwind(AssertUnwindSafe(|| drop(panic))) {
            Ok(()) => return,
            Err(secondary) => panic = secondary,
        }
    }
    abort_after_repeated_payload_panic(panic);
}

#[cfg_attr(coverage_nightly, coverage(off))] // Intentional process termination is covered by child-process contracts.
fn abort_after_repeated_payload_panic(_panic: TaskPanicPayload) -> ! {
    std::process::abort();
}
