// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

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
    let state = unsafe { &*data.cast::<CloneHook>() };
    let hook = state.hook.lock().expect("the clone hook mutex must not be poisoned").take();
    if let Some(hook) = hook {
        hook();
    }
    // SAFETY: the source RawWaker still owns a strong reference, and the new
    // RawWaker takes ownership of the incremented reference.
    unsafe { Arc::increment_strong_count(data.cast::<CloneHook>()) };
    RawWaker::new(data, &VTABLE)
}

unsafe fn wake(data: *const ()) {
    // SAFETY: consuming wake owns one Arc strong reference.
    drop(unsafe { Arc::<CloneHook>::from_raw(data.cast()) });
}

// This test waker observes waiter registration through Waker::clone. Waking is
// intentionally inert so it cannot trigger the registration hook.
unsafe fn wake_by_ref(_data: *const ()) {}

unsafe fn drop_waker(data: *const ()) {
    // SAFETY: dropping the RawWaker releases its owned Arc strong reference.
    drop(unsafe { Arc::<CloneHook>::from_raw(data.cast()) });
}

static VTABLE: RawWakerVTable = RawWakerVTable::new(clone_waker, wake, wake_by_ref, drop_waker);
