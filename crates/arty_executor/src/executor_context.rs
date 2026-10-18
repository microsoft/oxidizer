// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::thread::ThreadId;

use crate::{TaskPointer, TaskRef};

thread_local! {
    static CURRENT: RefCell<State> = const {
        RefCell::new(State {
            owner: None,
            awakened: VecDeque::new(),
            executing: false,
        })
    };
}

struct State {
    owner: Option<ThreadId>,
    awakened: VecDeque<TaskRef>,
    executing: bool,
}

// Initial capacity for the synchronization-free owner-thread wake queue.
#[cfg(not(miri))]
pub(crate) const AWAKENED_CAPACITY: usize = 1024;

// Miri performance scales with memory access count.
#[cfg(miri)]
pub(crate) const AWAKENED_CAPACITY: usize = 4;

pub(crate) fn register(owner: ThreadId) {
    CURRENT.with_borrow_mut(|current| {
        assert!(current.owner.is_none(), "only one live Executor may exist on a thread");
        current.owner = Some(owner);
        current.awakened = VecDeque::with_capacity(AWAKENED_CAPACITY);
        current.executing = false;
    });
}

pub(crate) fn unregister(owner: ThreadId) {
    _ = CURRENT.try_with(|current| {
        let mut current = current.borrow_mut();
        assert_eq!(current.owner, Some(owner), "the current thread must unregister its own Executor");
        current.owner = None;
        current.awakened.clear();
        current.executing = false;
    });
}

pub(crate) fn is_current(owner: ThreadId) -> bool {
    CURRENT
        .try_with(|current| {
            let current = current.borrow();
            current.owner == Some(owner)
        })
        .unwrap_or(false)
}

pub(crate) fn enter_cycle(owner: ThreadId) -> CycleGuard {
    CURRENT.with_borrow_mut(|current| {
        assert_eq!(current.owner, Some(owner));
        assert!(!current.executing, "Executor::execute_cycle cannot be called reentrantly");
        current.executing = true;
    });
    CycleGuard
}

pub(crate) fn is_executing(owner: ThreadId) -> bool {
    CURRENT.with_borrow(|current| current.owner == Some(owner) && current.executing)
}

pub(crate) fn enqueue(owner: ThreadId, task_ref: TaskRef) -> Result<(), TaskRef> {
    if !is_current(owner) {
        return Err(task_ref);
    }
    CURRENT.with_borrow_mut(|current| current.awakened.push_back(task_ref));
    Ok(())
}

pub(crate) fn append_wakes(owner: ThreadId, target: &mut VecDeque<TaskRef>) {
    if is_current(owner) {
        CURRENT.with_borrow_mut(|current| target.append(&mut current.awakened));
    }
}

pub(crate) fn has_wakes(owner: ThreadId) -> bool {
    is_current(owner) && CURRENT.with_borrow(|current| !current.awakened.is_empty())
}

pub(crate) fn remove_wake(owner: ThreadId, task_ref: TaskPointer) {
    if is_current(owner) {
        CURRENT.with_borrow_mut(|current| {
            current.awakened.retain(|queued| queued.pointer() != task_ref);
        });
    }
}

pub(crate) fn clear_wakes(owner: ThreadId) {
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
