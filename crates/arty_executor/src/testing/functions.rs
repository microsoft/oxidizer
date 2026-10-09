// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::task::Waker;
use std::time::Duration;
use std::{env, thread};

use infinity_pool::RawBlindPool;
use plurality::{Box as PoolBox, Pool};
use scopeguard::{Always, ScopeGuard};

use crate::wake::{WakeShared, WakerState};
use crate::{CycleOutcome, Executor, RawPooledCastTypeErasedTask, TaskRef, TypeErasedTask, WakeSignal};

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

/// Benchmark-only fixture comparing pooled wake-state churn with fresh `Arc` allocation.
#[derive(Debug)]
pub struct WakerStateAllocationProbe {
    pool: Pool<WakerState>,
    pooled: Option<PoolBox<WakerState>>,
    fresh: Option<Arc<WakerState>>,
    shared: Arc<WakeShared>,
    task_ref: TaskRef,
    _task_storage: RawBlindPool,
}

impl WakerStateAllocationProbe {
    /// Creates a probe and warms one pool slot so later pooled replacements reuse it.
    #[must_use]
    pub fn new() -> Self {
        let mut task_storage = RawBlindPool::new();
        let task = task_storage.insert(AllocationProbeTask(0));
        let task_ref = TaskRef::new(
            // SAFETY: The probe storage owns the task for the complete lifetime of every state
            // carrying this opaque reference, and the benchmark never dereferences it.
            unsafe { task.cast_type_erased_task() }.into_shared(),
        );
        let mut probe = Self {
            pool: Pool::new(),
            pooled: None,
            fresh: None,
            shared: Arc::new(WakeShared::new(
                Arc::new(Mutex::new(VecDeque::new())),
                Arc::new(AtomicBool::new(false)),
                Waker::noop().clone(),
            )),
            task_ref,
            _task_storage: task_storage,
        };
        probe.replace_pooled();
        probe.pooled = None;
        probe
    }

    fn state(&self) -> WakerState {
        WakerState::new(Arc::downgrade(&self.shared), self.task_ref)
    }

    /// Replaces one pooled wake state, returning the previous slot before reusing capacity.
    pub fn replace_pooled(&mut self) {
        self.pooled = None;
        let state = self.state();
        self.pooled = Some(self.pool.alloc_box(state));
    }

    /// Replaces one freshly allocated `Arc` wake state.
    pub fn replace_fresh_arc(&mut self) {
        self.fresh = None;
        let state = self.state();
        self.fresh = Some(Arc::new(state));
    }
}

impl Default for WakerStateAllocationProbe {
    fn default() -> Self {
        Self::new()
    }
}

struct AllocationProbeTask(u8);

impl TypeErasedTask for AllocationProbeTask {
    fn poll(self: Pin<&Self>) -> std::task::Poll<()> {
        unreachable!("the allocation probe never polls its opaque task reference")
    }

    fn is_inert(&self) -> bool {
        _ = self.0;
        true
    }

    fn consume_awakened(&self) -> bool {
        false
    }

    fn clear_queued_notification(&self) {}

    fn abort(self: Pin<&Self>) {}

    unsafe fn initialize(self: Pin<&Self>, _wake_signal: WakeSignal) {
        unreachable!("the allocation probe never initializes its opaque task reference")
    }

    #[cfg(all(debug_assertions, test))]
    fn inspect_waker_backtraces(&self, _f: &mut dyn FnMut(&std::backtrace::Backtrace)) {}
}
