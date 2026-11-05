// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::atomic::Ordering;

#[inline]
pub(super) fn fence(order: Ordering) {
    // #[gamma::skip(stmt.delete_call, reason = "The native memory-ordering intrinsic needs concurrent model checking; deterministic tests verify registration, notification and broadcast fence dispatch at the original call sites.")]
    std::sync::atomic::fence(order);
    #[cfg(test)]
    record_fence();
}

#[cfg(test)]
mod observation {
    use std::cell::{Cell, RefCell};

    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub(in crate::sync) struct Effects {
        pub(in crate::sync) locks: usize,
        pub(in crate::sync) fences: usize,
        pub(in crate::sync) wake_dispatches: usize,
        pub(in crate::sync) slow_releases: usize,
    }

    thread_local! {
        static EFFECTS: Cell<Effects> = const {
            Cell::new(Effects { locks: 0, fences: 0, wake_dispatches: 0, slow_releases: 0 })
        };
        static PARK: RefCell<Option<Box<dyn FnMut()>>> = const { RefCell::new(None) };
        static RETRIES: Cell<Option<usize>> = const { Cell::new(None) };
    }

    pub(in crate::sync) fn take() -> Effects {
        EFFECTS.replace(Effects::default())
    }

    pub(in crate::sync) fn record_lock() {
        let mut effects = EFFECTS.get();
        effects.locks += 1;
        EFFECTS.set(effects);
    }

    pub(in crate::sync) fn record_fence() {
        let mut effects = EFFECTS.get();
        effects.fences += 1;
        EFFECTS.set(effects);
    }

    pub(in crate::sync) fn record_wake_dispatch() {
        let mut effects = EFFECTS.get();
        effects.wake_dispatches += 1;
        EFFECTS.set(effects);
    }

    pub(in crate::sync) fn record_slow_release() {
        let mut effects = EFFECTS.get();
        effects.slow_releases += 1;
        EFFECTS.set(effects);
    }

    pub(in crate::sync) fn park_is_injected() -> bool {
        PARK.with_borrow(Option::is_some)
    }

    pub(in crate::sync) fn injected_park() -> bool {
        PARK.with_borrow_mut(|park| {
            if let Some(park) = park {
                park();
                true
            } else {
                false
            }
        })
    }

    pub(in crate::sync) fn with_park(park: impl FnMut() + 'static, run: impl FnOnce()) {
        struct Reset(Option<Box<dyn FnMut()>>);
        impl Drop for Reset {
            fn drop(&mut self) {
                PARK.with_borrow_mut(|park| *park = self.0.take());
            }
        }
        let _reset = Reset(PARK.replace(Some(Box::new(park))));
        run();
    }

    pub(in crate::sync) fn check_retry_budget() {
        if let Some(remaining) = RETRIES.get() {
            assert!(remaining > 0, "native retry must dispatch its injected wait before retrying");
            RETRIES.set(Some(remaining - 1));
        }
    }

    pub(in crate::sync) fn with_retry_budget(budget: usize, run: impl FnOnce()) {
        struct Reset(Option<usize>);
        impl Drop for Reset {
            fn drop(&mut self) {
                RETRIES.set(self.0);
            }
        }
        let _reset = Reset(RETRIES.replace(Some(budget)));
        run();
    }
}

#[cfg(test)]
pub(super) use observation::{
    check_retry_budget, injected_park, park_is_injected, record_fence, record_lock, record_slow_release, record_wake_dispatch, take,
    with_park, with_retry_budget,
};

#[cfg(test)]
mod tests {
    use std::task::{Context, Waker};

    use crate::sync::Async;
    use crate::sync::lock::RwLock;
    use crate::sync::mutex::Mutex;
    use crate::sync::notify::Notify;

    #[test]
    fn nov4_async_owner_release_avoids_dispatching_an_unregistered_wait_queue() {
        let mutex = Mutex::<u32, Async>::new(3);
        let owner = mutex.lock();
        super::take();
        drop(owner);
        assert_eq!(super::take().wake_dispatches, 0);

        let lock = RwLock::<u32, Async>::new(7);
        let first = lock.read();
        let second = lock.read();
        super::take();
        drop(first);
        drop(second);
        assert_eq!(super::take().wake_dispatches, 0);
        let owner = lock.write();
        super::take();
        drop(owner);
        assert_eq!(super::take().wake_dispatches, 0);
    }

    #[test]
    fn nov4_notification_acquires_the_initialized_waiter_sidecar_before_delivery() {
        let notify = Notify::<Async>::new();
        let mut waiting = std::pin::pin!(notify.wait_async());
        assert!(waiting.as_mut().poll(&mut Context::from_waker(Waker::noop())).is_pending());
        super::take();
        notify.notify_one();
        assert_eq!(super::take().fences, 1);
        assert!(waiting.as_mut().poll(&mut Context::from_waker(Waker::noop())).is_ready());
    }
}
