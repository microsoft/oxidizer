// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::cell::UnsafeCell;
use std::mem;
use std::pin::Pin;
use std::rc::Rc;
use std::task::Poll::{Pending, Ready};
use std::task::{self, Waker};

use negative_impl::negative_impl;

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
/// Both the sender and receiver must be on the same thread - this is a single-threaded event.
pub fn new_inefficient<T>() -> (InefficientSender<T>, InefficientReceiver<T>) {
    let core = Rc::new(OnceEvent {
        state: UnsafeCell::new(EventState::NotSet),
    });

    (
        InefficientSender {
            event: Rc::clone(&core),
        },
        InefficientReceiver { event: core },
    )
}

#[derive(Debug)]
struct OnceEvent<T> {
    // We only have a get() and a set() that access the state and we guarantee this happens on the
    // same thread, so there is no point in wasting cycles on borrow counting at runtime with
    // RefCell - there cannot be any concurrent access to this field.
    state: UnsafeCell<EventState<T>>,
}

impl<T> OnceEvent<T> {
    #[cfg_attr(test, mutants::skip)] // Critical primitive - causes test timeouts if tampered.
    fn set(&self, result: T) {
        // SAFETY: See comments on field.
        let state = unsafe { &mut *self.state.get() };

        match &*state {
            EventState::NotSet => {
                *state = EventState::Set(ValueKind::Real(result));
            }
            EventState::Awaiting(_) => {
                let previous_state =
                    mem::replace(&mut *state, EventState::Set(ValueKind::Real(result)));

                match previous_state {
                    EventState::Awaiting(waker) => waker.wake(),
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

    /// We are intended to be polled via `Future::poll`, so we have an equivalent signature here.
    ///
    /// # Panics
    ///
    /// Panics if the result has already been consumed.
    ///
    /// Panics if the sender has been dropped without setting the result.
    fn poll(&self, waker: &Waker) -> Option<T> {
        // SAFETY: See comments on field.
        let state = unsafe { &mut *self.state.get() };

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
        // SAFETY: See comments on field.
        let state = unsafe { &mut *self.state.get() };

        match &*state {
            EventState::NotSet => {
                *state = EventState::Set(ValueKind::Disconnected);
            }
            EventState::Awaiting(_) => {
                let previous_state =
                    mem::replace(&mut *state, EventState::Set(ValueKind::Disconnected));

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

#[negative_impl]
impl<T> !Send for OnceEvent<T> {}
#[negative_impl]
impl<T> !Sync for OnceEvent<T> {}

// The below is an implementation that relies on Rc to share state. This is inefficient and
// alternative implementations will be introduced in 2025 when we start performance optimizations.

#[derive(Debug)]
pub struct InefficientSender<T> {
    event: Rc<OnceEvent<T>>,
}

impl<T> InefficientSender<T> {
    #[cfg_attr(test, mutants::skip)] // Critical primitive - causes test timeouts if tampered.
    pub fn set(self, result: T) {
        self.event.set(result);
    }
}

impl<T> Drop for InefficientSender<T> {
    fn drop(&mut self) {
        self.event.sender_dropped();
    }
}

#[derive(Debug)]
pub struct InefficientReceiver<T> {
    event: Rc<OnceEvent<T>>,
}

impl<T> Future for InefficientReceiver<T> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> task::Poll<Self::Output> {
        let result = self.event.poll(cx.waker());

        result.map_or(Pending, |result| Ready(result))
    }
}

#[cfg(test)]
mod tests {
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

    #[test]
    fn get_twice_inefficient() {
        let (_sender, mut receiver) = new_inefficient::<()>();

        let cx = &mut task::Context::from_waker(noop_waker_ref());

        let result = receiver.poll_unpin(cx);
        assert_eq!(result, task::Poll::Pending);

        let cx = &mut task::Context::from_waker(noop_waker_ref());

        let result = receiver.poll_unpin(cx);
        assert_eq!(result, task::Poll::Pending);
    }
}