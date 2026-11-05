// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use core::sync::atomic::Ordering;

#[inline]
pub(crate) fn fence(order: Ordering) {
    core::sync::atomic::fence(order);
    #[cfg(test)]
    trace::record(order);
}

#[cfg(test)]
pub(crate) mod trace {
    use core::sync::atomic::Ordering;

    std::thread_local! {
        static EVENTS: std::cell::RefCell<Option<std::vec::Vec<Ordering>>> = const { std::cell::RefCell::new(None) };
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
                *events = Some(std::vec::Vec::new());
            });
            Self {
                thread: std::thread::current().id(),
            }
        }

        pub(crate) fn events(&self) -> std::vec::Vec<Ordering> {
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

#[cfg(test)]
mod tests {
    use core::sync::atomic::Ordering;

    use super::trace::Trace;
    use crate::BytesView;
    use crate::mem::{BlockSize, GlobalPool};

    #[test]
    fn gamma_nov6_relaxed_fence_is_rejected() {
        assert!(std::panic::catch_unwind(|| super::fence(Ordering::Relaxed)).is_err());
    }

    #[test]
    fn gamma_nov4_vec_and_bytes_last_owners_acquire_only_after_the_nonfinal_release() {
        for view in [
            BytesView::from(std::vec![11, 23, 37]),
            BytesView::from(bytes::Bytes::from(std::vec![41, 53, 67])),
        ] {
            let other = view.clone();
            let trace = Trace::start();
            drop(other);
            assert!(trace.events().is_empty());
            drop(view);
            assert_eq!(trace.events(), [Ordering::Acquire]);
        }
    }

    #[test]
    fn gamma_nov4_pool_and_test_blocks_acquire_only_on_final_release() {
        let pool = GlobalPool::new();
        let view = BytesView::copied_from_slice(b"retained", &pool);
        let other = view.clone();
        {
            let trace = Trace::start();
            drop(other);
            assert!(trace.events().is_empty());
            drop(view);
            assert_eq!(trace.events(), [Ordering::Acquire]);
        }
        let block = crate::mem::testing::std_alloc_block::allocate(core::num::NonZero::<BlockSize>::new(17).unwrap());
        let builder = block.into_span_builder();
        let other = builder.block().clone();
        let trace = Trace::start();
        drop(other);
        assert!(trace.events().is_empty());
        drop(builder);
        assert_eq!(trace.events(), [Ordering::Acquire]);
    }
}
