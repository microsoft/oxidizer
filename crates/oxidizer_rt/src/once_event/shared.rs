// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::mem;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{self, Waker};

use crate::ERR_POISONED_LOCK;

/// Creates an asynchronous event that can be triggered at most once to deliver a value of type T
/// to at most one listener awaiting that value.
///
/// Usage:
///
/// 1. Obtain a sender and receiver pair via this function.
/// 2. Call `set()` or `poll()` at most once to read or write the value through the sender/receiver.
///
/// # Efficiency
///
/// Event notifications are triggered instantly via waker if a listener is already awaiting, and
/// the result is delivered instantly if the listener starts after the result is set.
///
/// In future iterations, different implementations of the event will exist, with different
/// performance characteristics and storage models. This specific implementation aims for
/// simplicity at the expense of performance and efficiency.
///
/// # Disconnected senders
///
/// If the sender is destroyed without setting the event, the receiver will panic.
///
/// The intended use case for this type is to signal events that are always triggered under normal
/// operating conditions. Under exceptional conditions (e.g. sender encountered a panic), it may be
/// unavoidable to not trigger the event and simply drop the sender. To avoid the receiver awaiting
/// forever (which would hang the process/test), it will panic.
///
/// # Thread safety
///
/// The sender and receiver may be on any thread - this is a thread-safe event.
pub fn new_inefficient<T>() -> (InefficientSender<T>, InefficientReceiver<T>) {
    let core = Arc::new(OnceEventShared {
        state: Mutex::new(EventState::NotSet),
    });

    (
        InefficientSender {
            event: Arc::clone(&core),
        },
        InefficientReceiver { event: core },
    )
}

/// A thread-safe asynchronous event that can be triggered at most once to deliver a value of type T
/// to at most one listener awaiting that value.
///
/// Usage:
///
/// 1. Create the event via `new()`. This will return a sender and receiver pair.
/// 2. Call `set()` or `poll()` at most once to read or write the value through the sender/receiver.
///
/// # Efficiency
///
/// Event notifications are triggered instantly via waker if a listener is already awaiting, and
/// the result is delivered instantly if the listener starts after the result is set.
///
/// # Thread safety
///
/// This type is thread-safe. For a single-threaded version, see `OnceEvent`.
#[derive(Debug)]
struct OnceEventShared<T> {
    state: Mutex<EventState<T>>,
}

impl<T> OnceEventShared<T> {
    #[cfg_attr(test, mutants::skip)] // Critical primitive - causes test timeouts if tampered.
    fn set(&self, result: T) {
        let mut waker: Option<Waker> = None;

        {
            let mut state = self.state.lock().expect(ERR_POISONED_LOCK);

            match &*state {
                EventState::NotSet => {
                    *state = EventState::Set(ValueKind::Real(result));
                }
                EventState::Awaiting(_) => {
                    let previous_state =
                        mem::replace(&mut *state, EventState::Set(ValueKind::Real(result)));

                    drop(state);

                    match previous_state {
                        EventState::Awaiting(w) => waker = Some(w),
                        _ => unreachable!("we are re-matching an already matched pattern"),
                    }
                }
                EventState::Set(_) => {
                    panic!("result already set");
                }
                EventState::Consumed => {
                    panic!("result already consumed");
                }
            }
        }

        // We perform the wakeup outside the lock to avoid unnecessary contention if the receiver
        // of the result wakes up instantly and we have not released our lock yet.
        if let Some(waker) = waker {
            waker.wake();
        }
    }

    // We are intended to be polled via Future::poll, so we have an equivalent signature here.
    #[cfg_attr(test, mutants::skip)] // Critical for code execution to occur in async contexts.
    fn poll(&self, waker: &Waker) -> Option<T> {
        let mut state = self.state.lock().expect(ERR_POISONED_LOCK);

        match &*state {
            EventState::NotSet => {
                *state = EventState::Awaiting(waker.clone());
                None
            }
            EventState::Awaiting(_) => {
                // This is permitted by the Future API contract, in which case only the waker
                // from the most recent poll should be woken up when the result is available.
                *state = EventState::Awaiting(waker.clone());
                None
            }
            EventState::Set(_) => {
                let previous_state = mem::replace(&mut *state, EventState::Consumed);

                drop(state);

                match previous_state {
                    EventState::Set(result) => match result {
                        ValueKind::Real(result) => Some(result),
                        ValueKind::Disconnected => panic!("sender dropped without setting result"),
                    },
                    _ => unreachable!("we are re-matching an already matched pattern"),
                }
            }
            EventState::Consumed => {
                // We do not want to keep a copy of the result around, so we can only return it once.
                // The futures API contract allows us to panic in this situation.
                panic!("event polled after result was already consumed");
            }
        }
    }

    fn sender_dropped(&self) {
        let mut state = self.state.lock().expect(ERR_POISONED_LOCK);

        match &*state {
            EventState::NotSet => {
                *state = EventState::Set(ValueKind::Disconnected);
            }
            EventState::Awaiting(_) => {
                let previous_state =
                    mem::replace(&mut *state, EventState::Set(ValueKind::Disconnected));

                drop(state);

                match previous_state {
                    EventState::Awaiting(waker) => waker.wake(),
                    _ => unreachable!("we are re-matching an already matched pattern"),
                }
            }
            _ => {}
        }
    }
}

#[derive(Debug)]
enum EventState<T> {
    /// The event has not been set and nobody is listening for a result.
    NotSet,

    /// The event has not been set and someone is listening for a result.
    Awaiting(Waker),

    /// The event has been set but nobody has yet started listening.
    Set(ValueKind<T>),

    /// The event has been set and the result has been consumed.
    Consumed,
}

#[derive(Debug)]
enum ValueKind<T> {
    /// The event has been set to a real value.
    Real(T),

    /// The sender has been dropped without ever providing a value.
    Disconnected,
}

// The below is an implementation that relies on Rc to share state. This is inefficient and
// alternative implementations will be introduced in 2025 when we start performance optimizations.

#[derive(Debug)]
pub struct InefficientSender<T> {
    event: Arc<OnceEventShared<T>>,
}

impl<T> InefficientSender<T> {
    #[cfg_attr(test, mutants::skip)] // Critical primitive - causes test timeouts if tampered.
    pub fn set(self, result: T) {
        self.event.set(result);
    }
}

#[derive(Debug)]
pub struct InefficientReceiver<T> {
    event: Arc<OnceEventShared<T>>,
}

impl<T> Future for InefficientReceiver<T> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> task::Poll<Self::Output> {
        let result = self.event.poll(cx.waker());

        result.map_or_else(|| task::Poll::Pending, |result| task::Poll::Ready(result))
    }
}

impl<T> Drop for InefficientSender<T> {
    fn drop(&mut self) {
        self.event.sender_dropped();
    }
}

#[cfg(test)]
mod tests {
    use std::thread;

    use futures::FutureExt;
    use futures::task::noop_waker_ref;

    use super::*;

    #[test]
    fn get_after_set_inefficient() {
        let (sender, mut receiver) = new_inefficient();

        sender.set(42);

        let cx = &mut task::Context::from_waker(noop_waker_ref());

        let result = receiver.poll_unpin(cx);
        assert_eq!(result, task::Poll::Ready(42));
    }

    #[test]
    fn get_after_set_inefficient_multithreaded() {
        let (sender, mut receiver) = new_inefficient();

        thread::spawn(move || {
            sender.set(42);
        })
        .join()
        .unwrap();

        thread::spawn(move || {
            let cx = &mut task::Context::from_waker(noop_waker_ref());

            let result = receiver.poll_unpin(cx);
            assert_eq!(result, task::Poll::Ready(42));
        })
        .join()
        .unwrap();
    }

    #[test]
    fn get_before_set_inefficient() {
        let (sender, mut receiver) = new_inefficient();

        let cx = &mut task::Context::from_waker(noop_waker_ref());

        let result = receiver.poll_unpin(cx);
        assert_eq!(result, task::Poll::Pending);

        sender.set(42);

        let result = receiver.poll_unpin(cx);
        assert_eq!(result, task::Poll::Ready(42));
    }

    #[test]
    fn get_before_set_inefficient_multithreaded() {
        let (sender, mut receiver) = new_inefficient();

        thread::spawn(move || {
            let cx = &mut task::Context::from_waker(noop_waker_ref());

            let result = receiver.poll_unpin(cx);
            assert_eq!(result, task::Poll::Pending);

            sender.set(42);

            let result = receiver.poll_unpin(cx);
            assert_eq!(result, task::Poll::Ready(42));
        })
        .join()
        .unwrap();
    }

    #[test]
    #[should_panic]
    #[expect(
        clippy::should_panic_without_expect,
        reason = "TODO: Add an expected string to [should_panic]"
    )]
    fn get_after_dropped_sender_inefficient() {
        let (sender, mut receiver) = new_inefficient::<()>();

        drop(sender);

        let cx = &mut task::Context::from_waker(noop_waker_ref());

        _ = receiver.poll_unpin(cx);
    }

    #[test]
    fn set_after_dropped_receiver_inefficient() {
        let (sender, receiver) = new_inefficient();

        drop(receiver);

        sender.set(42);
    }
}