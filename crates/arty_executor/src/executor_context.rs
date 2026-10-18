// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::cell::RefCell;
use std::collections::VecDeque;

use crate::{TaskPointer, TaskRef, WakeShared};

thread_local! {
    static CURRENT: RefCell<State> = const {
        RefCell::new(State {
            owner: std::ptr::null(),
            awakened: VecDeque::new(),
            executing: false,
        })
    };
}

struct State {
    owner: *const WakeShared,
    awakened: VecDeque<TaskRef>,
    executing: bool,
}

// Initial capacity for the synchronization-free owner-thread wake queue.
#[cfg(not(miri))]
pub(crate) const AWAKENED_CAPACITY: usize = 1024;

// Miri performance scales with memory access count.
#[cfg(miri)]
pub(crate) const AWAKENED_CAPACITY: usize = 4;

pub(crate) fn register(owner: &WakeShared) {
    let owner = std::ptr::from_ref(owner);
    CURRENT.with_borrow_mut(|current| {
        assert!(current.owner.is_null(), "only one live Executor may exist on a thread");
        current.owner = owner;
        current.awakened = VecDeque::with_capacity(AWAKENED_CAPACITY);
        current.executing = false;
    });
}

pub(crate) fn unregister(owner: &WakeShared) {
    let owner = std::ptr::from_ref(owner);
    _ = CURRENT.try_with(|current| {
        let mut current = current.borrow_mut();
        assert_eq!(current.owner, owner, "the current thread must unregister its own Executor");
        current.owner = std::ptr::null();
        current.awakened.clear();
        current.executing = false;
    });
}

pub(crate) fn is_current(owner: &WakeShared) -> bool {
    let owner = std::ptr::from_ref(owner);
    CURRENT
        .try_with(|current| {
            let current = current.borrow();
            current.owner == owner || cfg!(test) && current.owner.is_null()
        })
        .unwrap_or(false)
}

pub(crate) fn enter_cycle(owner: &WakeShared) -> CycleGuard {
    let owner = std::ptr::from_ref(owner);
    CURRENT.with_borrow_mut(|current| {
        assert_eq!(current.owner, owner);
        assert!(!current.executing, "Executor::execute_cycle cannot be called reentrantly");
        current.executing = true;
    });
    CycleGuard(owner)
}

pub(crate) fn is_executing(owner: &WakeShared) -> bool {
    let owner = std::ptr::from_ref(owner);
    CURRENT.with_borrow(|current| current.owner == owner && current.executing)
}

pub(crate) fn enqueue(owner: &WakeShared, task_ref: TaskRef) -> Result<(), TaskRef> {
    if !is_current(owner) {
        return Err(task_ref);
    }
    CURRENT.with_borrow_mut(|current| current.awakened.push_back(task_ref));
    Ok(())
}

pub(crate) fn append_wakes(owner: &WakeShared, target: &mut VecDeque<TaskRef>) {
    if is_current(owner) {
        CURRENT.with_borrow_mut(|current| target.append(&mut current.awakened));
    }
}

pub(crate) fn has_wakes(owner: &WakeShared) -> bool {
    is_current(owner) && CURRENT.with_borrow(|current| !current.awakened.is_empty())
}

pub(crate) fn remove_wake(owner: &WakeShared, task_ref: TaskPointer) {
    if is_current(owner) {
        CURRENT.with_borrow_mut(|current| {
            current.awakened.retain(|queued| queued.pointer() != task_ref);
        });
    }
}

pub(crate) fn clear_wakes(owner: &WakeShared) {
    if is_current(owner) {
        CURRENT.with_borrow_mut(|current| current.awakened.clear());
    }
}

pub(crate) struct CycleGuard(*const WakeShared);

impl Drop for CycleGuard {
    fn drop(&mut self) {
        _ = CURRENT.try_with(|current| {
            let mut current = current.borrow_mut();
            if current.owner == self.0 {
                current.executing = false;
            }
        });
    }
}
