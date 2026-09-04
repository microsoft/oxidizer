// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::mem::ManuallyDrop;
use std::sync::{Arc, Mutex};
use std::task::{RawWaker, RawWakerVTable, Waker};

struct CloneHook {
    hook: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

pub(crate) fn clone_hook_waker(hook: impl FnOnce() + Send + 'static) -> Waker {
    let state = Arc::new(CloneHook {
        hook: Mutex::new(Some(Box::new(hook))),
    });
    let raw = RawWaker::new(Arc::into_raw(state).cast(), &VTABLE);
    // SAFETY: VTABLE maintains one Arc strong reference for every RawWaker.
    unsafe { Waker::from_raw(raw) }
}

unsafe fn clone_waker(data: *const ()) -> RawWaker {
    // SAFETY: data was created by Arc::into_raw in clone_hook_waker or this function.
    let state = ManuallyDrop::new(unsafe { Arc::<CloneHook>::from_raw(data.cast()) });
    let hook = state.hook.lock().expect("the clone hook mutex must not be poisoned").take();
    if let Some(hook) = hook {
        hook();
    }
    let clone = Arc::clone(&state);
    RawWaker::new(Arc::into_raw(clone).cast(), &VTABLE)
}

unsafe fn wake(data: *const ()) {
    // SAFETY: consuming wake owns one Arc strong reference.
    drop(unsafe { Arc::<CloneHook>::from_raw(data.cast()) });
}

unsafe fn wake_by_ref(_data: *const ()) {}

unsafe fn drop_waker(data: *const ()) {
    // SAFETY: dropping the RawWaker releases its owned Arc strong reference.
    drop(unsafe { Arc::<CloneHook>::from_raw(data.cast()) });
}

static VTABLE: RawWakerVTable = RawWakerVTable::new(clone_waker, wake, wake_by_ref, drop_waker);
