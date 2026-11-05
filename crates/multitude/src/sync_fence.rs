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
#[test]
fn gamma_nov6_relaxed_fence_is_rejected() {
    assert!(std::panic::catch_unwind(|| fence(Ordering::Relaxed)).is_err());
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

    pub(crate) fn events() -> std::vec::Vec<Ordering> {
        EVENTS.with_borrow(|events| events.as_ref().unwrap().clone())
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

    use super::trace::{self, Trace};

    struct Payload(std::sync::Arc<core::sync::atomic::AtomicUsize>);

    impl Drop for Payload {
        fn drop(&mut self) {
            assert_eq!(
                trace::events(),
                [Ordering::Acquire],
                "the final owner must acquire before reading or dropping payload state"
            );
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn gamma_nov4_arc_final_release_acquires_before_the_payload_destructor() {
        let arena = crate::Arena::new();
        let dropped = std::sync::Arc::new(core::sync::atomic::AtomicUsize::new(0));
        let value = arena.alloc_arc(Payload(std::sync::Arc::clone(&dropped)));
        let other = value.clone();
        let trace = Trace::start();
        drop(other);
        assert!(trace.events().is_empty());
        assert_eq!(dropped.load(Ordering::Relaxed), 0);
        drop(value);
        assert_eq!(trace.events(), [Ordering::Acquire]);
        assert_eq!(dropped.load(Ordering::Relaxed), 1);
    }

    #[cfg(feature = "bytesbuf")]
    #[test]
    fn gamma_nov4_arena_block_last_release_acquires_before_releasing_its_arc() {
        use bytesbuf::mem::Memory;
        let arena = crate::Arena::new();
        let mut buffer = arena.reserve(17);
        buffer.put_slice(*b"retained");
        let view = buffer.consume_all();
        drop(buffer);
        let other = view.clone();
        let trace = Trace::start();
        drop(other);
        assert!(trace.events().is_empty());
        drop(view);
        assert_eq!(trace.events(), [Ordering::Acquire, Ordering::Acquire]);
    }
}
