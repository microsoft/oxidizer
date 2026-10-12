// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::task::Waker;
use std::thread;

use infinity_pool::RawBlindPool;
use plurality::{Box as PoolBox, Pool};
use scopeguard::{Always, ScopeGuard};

use crate::wake::{WakeShared, WakerState};
use crate::{CycleOutcome, Executor, RawPooledCastTypeErasedTask, TaskRef, TypeErasedTask, WakeSignal};

/// Creates a new `Executor` with a scopeguard that ensures safe shutdown on drop.
#[must_use]
#[cfg_attr(test, mutants::skip)] // This is test logic, mutation here is unhelpful.
pub fn new_guarded_executor(owner_waker: Waker) -> ScopeGuard<Executor, fn(Executor), Always> {
    scopeguard::guard(Executor::builder().owner_waker(owner_waker).build(), |executor: Executor| {
        executor.begin_shutdown();

        while executor.execute_cycle() != CycleOutcome::Shutdown {
            thread::yield_now();
        }
    })
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
        let task = task_storage.insert(AllocationProbeTask { _nonzero: 0 });
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
    #[cfg_attr(test, mutants::skip)] // Benchmark operation; measured directly instead of unit-tested.
    pub fn replace_pooled(&mut self) {
        self.pooled = None;
        let state = self.state();
        self.pooled = Some(self.pool.alloc_box(state));
    }

    /// Replaces one freshly allocated `Arc` wake state.
    #[cfg_attr(test, mutants::skip)] // Benchmark comparison; measured directly instead of unit-tested.
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

struct AllocationProbeTask {
    _nonzero: u8,
}

impl TypeErasedTask for AllocationProbeTask {
    fn poll(self: Pin<&Self>) -> std::task::Poll<()> {
        unreachable!("the allocation probe never polls its opaque task reference")
    }

    #[cfg_attr(test, mutants::skip)] // Opaque benchmark task is never exercised as an executor task.
    fn consume_awakened(&self) -> bool {
        false
    }

    fn clear_queued_notification(&self) {}

    fn abort(self: Pin<&Self>) {}

    #[cfg_attr(test, mutants::skip)] // Opaque benchmark task is never initialized as an executor task.
    fn initialize(self: Pin<&Self>, _wake_signal: WakeSignal) {
        unreachable!("the allocation probe never initializes its opaque task reference")
    }

    #[cfg(all(debug_assertions, test))]
    fn inspect_waker_backtraces(&self, _f: &mut dyn FnMut(&std::backtrace::Backtrace)) {}
}
