// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::sync::Arc;

use crate::{TaskHandle, TaskPtr};

thread_local! {
    static CURRENT: RefCell<State> = const {
        RefCell::new(State {
            owner: None,
            awakened: VecDeque::new(),
            executing: false,
        })
    };
}

#[derive(Clone, Debug)]
pub(crate) struct ExecutorId {
    identity: Arc<()>,
}

impl ExecutorId {
    pub(crate) fn new() -> Self {
        Self { identity: Arc::new(()) }
    }

    fn matches(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.identity, &other.identity)
    }
}

struct State {
    owner: Option<ExecutorId>,
    awakened: VecDeque<TaskHandle>,
    executing: bool,
}

// Initial capacity for the synchronization-free owner-thread wake queue.
#[cfg(not(miri))]
pub(crate) const AWAKENED_CAPACITY: usize = 1024;

// Miri performance scales with memory access count.
#[cfg(miri)]
pub(crate) const AWAKENED_CAPACITY: usize = 4;

pub(crate) fn register(owner: ExecutorId) {
    CURRENT.with_borrow_mut(|current| {
        assert!(current.owner.is_none(), "only one live Executor may exist on a thread");
        current.owner = Some(owner);
        current.awakened = VecDeque::with_capacity(AWAKENED_CAPACITY);
        current.executing = false;
    });
}

pub(crate) fn unregister(owner: &ExecutorId) {
    _ = CURRENT.try_with(|current| {
        let mut current = current.borrow_mut();
        assert!(
            current.owner.as_ref().is_some_and(|current_owner| current_owner.matches(owner)),
            "the current thread must unregister its own Executor"
        );
        current.owner = None;
        current.awakened.clear();
        current.executing = false;
    });
}

pub(crate) fn is_current(owner: &ExecutorId) -> bool {
    CURRENT
        .try_with(|current| {
            let current = current.borrow();
            current.owner.as_ref().is_some_and(|current_owner| current_owner.matches(owner))
        })
        .unwrap_or(false)
}

pub(crate) fn enter_cycle(owner: &ExecutorId) -> CycleGuard {
    CURRENT.with_borrow_mut(|current| {
        assert!(current.owner.as_ref().is_some_and(|current_owner| current_owner.matches(owner)));
        assert!(!current.executing, "Executor::execute_cycle cannot be called reentrantly");
        current.executing = true;
    });
    CycleGuard
}

pub(crate) fn is_executing(owner: &ExecutorId) -> bool {
    CURRENT
        .try_with(|current| {
            let current = current.borrow();
            current.owner.as_ref().is_some_and(|current_owner| current_owner.matches(owner)) && current.executing
        })
        .unwrap_or(false)
}

pub(crate) fn enqueue(owner: &ExecutorId, task_handle: TaskHandle) -> Result<(), TaskHandle> {
    if !is_current(owner) {
        return Err(task_handle);
    }
    CURRENT.with_borrow_mut(|current| current.awakened.push_back(task_handle));
    Ok(())
}

pub(crate) fn append_wakes(owner: &ExecutorId, target: &mut VecDeque<TaskHandle>) {
    if is_current(owner) {
        CURRENT.with_borrow_mut(|current| target.append(&mut current.awakened));
    }
}

pub(crate) fn has_wakes(owner: &ExecutorId) -> bool {
    is_current(owner) && CURRENT.with_borrow(|current| !current.awakened.is_empty())
}

pub(crate) fn remove_wake(owner: &ExecutorId, task_ptr: TaskPtr) {
    if is_current(owner) {
        CURRENT.with_borrow_mut(|current| {
            current.awakened.retain(|queued| queued.as_ptr() != task_ptr);
        });
    }
}

pub(crate) fn clear_wakes(owner: &ExecutorId) {
    if is_current(owner) {
        CURRENT.with_borrow_mut(|current| current.awakened.clear());
    }
}

pub(crate) struct CycleGuard;

impl Drop for CycleGuard {
    fn drop(&mut self) {
        _ = CURRENT.try_with(|current| {
            let mut current = current.borrow_mut();
            current.executing = false;
        });
    }
}

#[cfg(test)]
mod tests {
    use plurality::MultiPool;

    use super::*;

    #[test]
    fn foreign_owner_operations_leave_local_wakes_unchanged() {
        std::thread::spawn(|| {
            let pool = MultiPool::new();
            let foreign_owner = ExecutorId::new();
            let task_handle = crate::allocate_test_task_in(&pool);
            let pointer = task_handle.as_ptr();
            let returned = enqueue(&foreign_owner, task_handle).unwrap_err();
            let mut target = VecDeque::new();

            append_wakes(&foreign_owner, &mut target);
            remove_wake(&foreign_owner, pointer);
            clear_wakes(&foreign_owner);

            assert!(target.is_empty());
            drop(returned);
            assert!(pool.is_empty());
        })
        .join()
        .unwrap();
    }

    #[test]
    fn cycle_state_and_queued_wakes_follow_owner_transitions() {
        std::thread::spawn(|| {
            let pool = MultiPool::new();
            let owner = ExecutorId::new();
            register(owner.clone());
            assert!(!is_executing(&owner));

            {
                let _cycle = enter_cycle(&owner);
                assert!(is_executing(&owner));
            }
            assert!(!is_executing(&owner));

            enqueue(&owner, crate::allocate_test_task_in(&pool)).unwrap();
            assert!(has_wakes(&owner));
            clear_wakes(&owner);
            assert!(!has_wakes(&owner));

            unregister(&owner);
            assert!(pool.is_empty());
        })
        .join()
        .unwrap();
    }
}
