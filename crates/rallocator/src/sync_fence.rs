// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::atomic::Ordering;

#[inline]
pub(crate) fn fence(order: Ordering) {
    #[cfg(not(test))]
    let atomic_fence = std::sync::atomic::fence;
    #[cfg(test)]
    let atomic_fence = trace::fence;
    atomic_fence(order);
}

#[cfg(test)]
pub(crate) mod trace {
    use std::sync::atomic::Ordering;

    std::thread_local! {
        static EVENTS: std::cell::RefCell<Option<Vec<Ordering>>> = const { std::cell::RefCell::new(None) };
    }

    pub(super) fn fence(order: Ordering) {
        std::sync::atomic::fence(order);
        record(order);
    }

    pub(crate) fn record(order: Ordering) {
        EVENTS.with_borrow_mut(|events| {
            if let Some(events) = events {
                events.push(order);
            }
        });
    }

    #[derive(Debug)]
    pub(crate) struct Trace {
        thread: std::thread::ThreadId,
    }

    impl Trace {
        pub(crate) fn start() -> Self {
            EVENTS.with_borrow_mut(|events| {
                assert!(events.is_none());
                *events = Some(Vec::new());
            });
            Self {
                thread: std::thread::current().id(),
            }
        }

        pub(crate) fn events(&self) -> Vec<Ordering> {
            assert_eq!(self.thread, std::thread::current().id());
            EVENTS.with_borrow(|events| events.as_ref().unwrap().clone())
        }
    }

    impl Drop for Trace {
        fn drop(&mut self) {
            EVENTS.with_borrow_mut(|events| *events = None);
        }
    }
}
