// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Caller-side handles for running actors.
//!
//! [`ActorRef`] sends messages and lifecycle requests to an actor that has
//! already been spawned.

use futures::TryFutureExt;

use crate::inbox::{Control, Sender, TrySendFailure};
use crate::{AskError, Message, Reply, TellError};

/// A message not admitted by [`ActorRef::try_tell`].
///
/// This value is result data, not an [`Error`](std::error::Error). Its public
/// variants let callers recover a message without adding `Debug`, `Sync`, or
/// `Clone` bounds to the message type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TryTellFailure<M> {
    /// The bounded actor inbox has no available capacity.
    Full(M),
    /// The actor inbox is closed.
    Closed(M),
}

impl<M> TryTellFailure<M> {
    /// Returns the message that was not sent.
    #[must_use]
    pub fn into_message(self) -> M {
        match self {
            Self::Full(message) | Self::Closed(message) => message,
        }
    }
}

impl<M> From<TrySendFailure<M>> for TryTellFailure<M> {
    fn from(error: TrySendFailure<M>) -> Self {
        match error {
            TrySendFailure::Full(message) => Self::Full(message),
            TrySendFailure::Closed(message) => Self::Closed(message),
        }
    }
}

/// Handle for sending messages to a running actor.
///
/// Cloning an [`ActorRef`] is cheap: clones share the same mailbox and control
/// channels, and do not clone or duplicate the actor itself.
#[derive(Debug)]
pub struct ActorRef<M: Message> {
    message_tx: Sender<M>,
    control_tx: Sender<Control>,
}

impl<M: Message> Clone for ActorRef<M> {
    fn clone(&self) -> Self {
        Self {
            message_tx: self.message_tx.clone(),
            control_tx: self.control_tx.clone(),
        }
    }
}

impl<M: Message> ActorRef<M> {
    pub(crate) const fn new(message_tx: Sender<M>, control_tx: Sender<Control>) -> Self {
        Self { message_tx, control_tx }
    }
}

impl<M> ActorRef<M>
where
    M: Message,
{
    /// Sends a fire-and-forget message to the actor.
    pub fn tell(&self, message: M) -> impl Future<Output = Result<(), TellError>> {
        // Forward the inbox send future instead of awaiting it in an `async fn`,
        // which would keep another copy of the message in its own state.
        self.message_tx.send(message).map_err(TellError::from)
    }

    /// Sends a message and returns it if the actor inbox rejects it.
    ///
    /// This method waits for capacity in a full bounded inbox. On success, the
    /// actor inbox owns the message.
    ///
    /// A send that is already waiting for capacity races with [`stop`](Self::stop)
    /// and [`drain`](Self::drain). If admission wins, this method returns `Ok(())`
    /// and the inbox owns the message. If closing wins, it returns the message.
    ///
    /// # Errors
    ///
    /// Returns the original message if the actor inbox rejects the send.
    pub fn tell_owned(&self, message: M) -> impl Future<Output = Result<(), M>> {
        self.message_tx.send(message).map_err(|error| error.0)
    }

    /// Tries to send a message without waiting for inbox capacity.
    ///
    /// On success, the actor inbox owns the message. On failure, the returned
    /// value distinguishes a full inbox from a closed inbox and retains the
    /// original message.
    ///
    /// # Errors
    ///
    /// Returns [`TryTellFailure::Full`] if the bounded inbox has no capacity,
    /// or [`TryTellFailure::Closed`] if the actor inbox is closed.
    pub fn try_tell(&self, message: M) -> Result<(), TryTellFailure<M>> {
        self.message_tx.try_send(message).map_err(Into::into)
    }

    /// Sends a zero-payload ask variant and waits for the reply.
    pub async fn ask<R>(&self, variant: fn(Reply<R>) -> M) -> Result<R, AskError>
    where
        R: Send + 'static,
    {
        let (reply, rx) = Reply::channel();
        self.tell(variant(reply)).await?;
        rx.await.map_err(Into::into)
    }

    /// Sends a one-payload ask variant and waits for the reply.
    pub async fn ask_with<A, R>(&self, variant: fn(A, Reply<R>) -> M, arg: A) -> Result<R, AskError>
    where
        A: Send + 'static,
        R: Send + 'static,
    {
        let (reply, rx) = Reply::channel();
        self.tell(variant(arg, reply)).await?;
        rx.await.map_err(Into::into)
    }

    /// Stops the actor after the current message finishes.
    pub async fn stop(&self) -> Result<(), TellError> {
        self.control_tx.stop().await?;
        self.message_tx.close();
        Ok(())
    }

    /// Closes the inbox, delivers queued messages, then stops the actor.
    pub async fn drain(&self) -> Result<(), TellError> {
        self.control_tx.drain().await?;
        self.message_tx.close();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::pin::pin;
    use std::task::{Context as TaskContext, Poll, Waker};

    use futures::executor::block_on;

    use super::{ActorRef, TryTellFailure};
    use crate::error::CanceledError;
    use crate::inbox::{Channel, Control, Signal, TryRecvError};
    use crate::{Reply, TellError};

    enum ControlOnlyMessage {}

    enum TestMessage {
        Ask(Reply<u8>),
        Lookup(u32, Reply<u8>),
    }

    #[derive(Debug)]
    struct NonCloneMessage {
        id: u8,
        identity: Box<u8>,
    }

    impl NonCloneMessage {
        fn new(id: u8) -> Self {
            Self {
                id,
                identity: Box::new(id),
            }
        }

        fn identity(&self) -> *const u8 {
            std::ptr::from_ref(self.identity.as_ref())
        }
    }

    #[derive(Clone, Copy)]
    enum CloseAction {
        Stop,
        Drain,
        Close,
    }

    fn assert_waiting_send_race(action: CloseAction) {
        let (message_tx, message_rx) = Channel::<NonCloneMessage>::bounded(1);
        let (control_tx, control_rx) = Channel::<Control>::unbounded();
        let actor_ref = ActorRef::new(message_tx, control_tx);

        actor_ref
            .try_tell(NonCloneMessage::new(1))
            .expect("first message should fill the inbox");

        let message = NonCloneMessage::new(2);
        let identity = message.identity();
        let mut send = pin!(actor_ref.tell_owned(message));
        let waker = Waker::noop();
        let mut cx = TaskContext::from_waker(waker);
        assert!(matches!(send.as_mut().poll(&mut cx), Poll::Pending));

        match action {
            CloseAction::Stop => {
                block_on(actor_ref.stop()).expect("stop should close the inbox");
                assert!(matches!(control_rx.try_recv(), Ok(Control::Signal(Signal::Stop))));
            }
            CloseAction::Drain => {
                block_on(actor_ref.drain()).expect("drain should close the inbox");
                assert!(matches!(control_rx.try_recv(), Ok(Control::Signal(Signal::Drain))));
            }
            CloseAction::Close => message_rx.close(),
        }

        let first = message_rx.try_recv().expect("first message should remain admitted");
        assert_eq!(first.id, 1);

        match block_on(send) {
            Ok(()) => {
                let delivered = message_rx.try_recv().expect("successful send should admit the message once");
                assert_eq!(delivered.identity(), identity);
            }
            Err(returned) => assert_eq!(returned.identity(), identity),
        }

        assert!(matches!(message_rx.try_recv(), Err(TryRecvError::Closed)));
    }

    #[test]
    fn tell_futures_add_no_message_copy_to_the_inbox_send() {
        type LargeMessage = [u8; 1024];

        let (message_tx, _message_rx) = Channel::<LargeMessage>::bounded(1);
        let (control_tx, _control_rx) = Channel::<Control>::unbounded();
        let actor_ref = ActorRef::new(message_tx, control_tx);

        let send = actor_ref.message_tx.send([0; 1024]);
        let tell = actor_ref.tell([0; 1024]);
        let tell_owned = actor_ref.tell_owned([0; 1024]);

        // Every send is on the hot path, so `tell` must forward the inbox send
        // future instead of keeping its own copy of the message.
        for size in [size_of_val(&tell), size_of_val(&tell_owned)] {
            assert!(
                size < size_of_val(&send) + size_of::<LargeMessage>(),
                "tell future is {size} bytes, inbox send future is {} bytes",
                size_of_val(&send)
            );
        }
    }

    #[test]
    fn tell_owned_waits_for_capacity_without_requiring_clone() {
        let (message_tx, message_rx) = Channel::<NonCloneMessage>::bounded(1);
        let (control_tx, _control_rx) = Channel::<Control>::unbounded();
        let actor_ref = ActorRef::new(message_tx, control_tx);
        actor_ref
            .try_tell(NonCloneMessage::new(1))
            .expect("first message should fill the inbox");

        let message = NonCloneMessage::new(2);
        let identity = message.identity();
        let mut send = pin!(actor_ref.tell_owned(message));
        let waker = Waker::noop();
        let mut cx = TaskContext::from_waker(waker);
        assert!(matches!(send.as_mut().poll(&mut cx), Poll::Pending));

        assert_eq!(message_rx.try_recv().expect("first message should leave the inbox").id, 1);
        block_on(send).expect("send should complete after capacity becomes available");

        let delivered = message_rx.try_recv().expect("second message should be admitted");
        assert_eq!(delivered.identity(), identity);
    }

    #[test]
    fn tell_owned_returns_exact_message_when_closed_while_waiting() {
        let (message_tx, message_rx) = Channel::<NonCloneMessage>::bounded(1);
        let (control_tx, _control_rx) = Channel::<Control>::unbounded();
        let actor_ref = ActorRef::new(message_tx, control_tx);
        actor_ref
            .try_tell(NonCloneMessage::new(1))
            .expect("first message should fill the inbox");

        let message = NonCloneMessage::new(2);
        let identity = message.identity();
        let mut send = pin!(actor_ref.tell_owned(message));
        let waker = Waker::noop();
        let mut cx = TaskContext::from_waker(waker);
        assert!(matches!(send.as_mut().poll(&mut cx), Poll::Pending));

        message_rx.close();
        drop(message_rx);

        let returned = block_on(send).expect_err("closed inbox should return the message");
        assert_eq!(returned.identity(), identity);
    }

    #[test]
    fn try_tell_returns_exact_message_when_full() {
        let (message_tx, _message_rx) = Channel::<NonCloneMessage>::bounded(1);
        let (control_tx, _control_rx) = Channel::<Control>::unbounded();
        let actor_ref = ActorRef::new(message_tx, control_tx);
        actor_ref
            .try_tell(NonCloneMessage::new(1))
            .expect("first message should fill the inbox");

        let message = NonCloneMessage::new(2);
        let identity = message.identity();
        let error = actor_ref.try_tell(message).expect_err("full inbox should return the message");

        let TryTellFailure::Full(returned) = error else {
            panic!("full inbox should report Full");
        };
        assert_eq!(returned.identity(), identity);
    }

    #[test]
    fn try_tell_returns_exact_message_when_closed() {
        let (message_tx, _message_rx) = Channel::<NonCloneMessage>::bounded(1);
        let (control_tx, _control_rx) = Channel::<Control>::unbounded();
        let actor_ref = ActorRef::new(message_tx, control_tx);
        block_on(actor_ref.stop()).expect("stop should close the inbox");

        let message = NonCloneMessage::new(2);
        let identity = message.identity();
        let error = actor_ref.try_tell(message).expect_err("closed inbox should return the message");

        let TryTellFailure::Closed(returned) = error else {
            panic!("closed inbox should report Closed");
        };
        assert_eq!(returned.identity(), identity);
    }

    #[test]
    fn tell_owned_stop_race_delivers_or_returns_message_once() {
        assert_waiting_send_race(CloseAction::Stop);
    }

    #[test]
    fn tell_owned_drain_race_delivers_or_returns_message_once() {
        assert_waiting_send_race(CloseAction::Drain);
    }

    #[test]
    fn tell_owned_close_race_delivers_or_returns_message_once() {
        assert_waiting_send_race(CloseAction::Close);
    }

    #[test]
    fn spammy_lifecycle_requests_all_queue() {
        let (message_tx, _message_rx) = Channel::<ControlOnlyMessage>::bounded(1);
        let (control_tx, control_rx) = Channel::<Control>::unbounded();
        let actor_ref = ActorRef::new(message_tx, control_tx);

        let waker = Waker::noop();
        let mut cx = TaskContext::from_waker(waker);

        for _ in 0..16 {
            let mut drain = pin!(actor_ref.drain());
            assert!(matches!(drain.as_mut().poll(&mut cx), Poll::Ready(Ok(()))));
        }

        let mut stop = pin!(actor_ref.stop());
        assert!(matches!(stop.as_mut().poll(&mut cx), Poll::Ready(Ok(()))));

        for _ in 0..16 {
            assert!(matches!(control_rx.try_recv(), Ok(Control::Signal(Signal::Drain))));
        }

        assert!(matches!(control_rx.try_recv(), Ok(Control::Signal(Signal::Stop))));
        assert!(matches!(control_rx.try_recv(), Err(TryRecvError::Empty)));
    }

    #[test]
    fn test_ask_returns_tell_error_when_actor_is_closed() {
        let (message_tx, message_rx) = Channel::<TestMessage>::bounded(1);
        let (control_tx, _control_rx) = Channel::<Control>::unbounded();
        let actor_ref = ActorRef::new(message_tx, control_tx);
        drop(message_rx);

        let result = block_on(actor_ref.ask(TestMessage::Ask)).unwrap_err();

        let _ = result.source().unwrap().downcast_ref::<TellError>().expect("should be TellError");
    }

    #[test]
    fn test_ask_returns_canceled_when_actor_drops_reply() {
        let (message_tx, message_rx) = Channel::<TestMessage>::bounded(1);
        let (control_tx, _control_rx) = Channel::<Control>::unbounded();
        let actor_ref = ActorRef::new(message_tx, control_tx);

        let result = block_on(async {
            let receive = async {
                let message = message_rx.recv().await.expect("message should arrive");
                let TestMessage::Ask(reply) = message else {
                    panic!("expected Ask variant");
                };
                drop(reply);
            };

            let call = actor_ref.ask(TestMessage::Ask);
            let ((), result) = futures::future::join(receive, call).await;
            result
        });

        let _ = result
            .unwrap_err()
            .source()
            .unwrap()
            .downcast_ref::<CanceledError>()
            .expect("should be CanceledError");
    }

    #[test]
    fn test_ask_with_sends_payload_and_receives_reply() {
        let (message_tx, message_rx) = Channel::<TestMessage>::bounded(1);
        let (control_tx, _control_rx) = Channel::<Control>::unbounded();
        let actor_ref = ActorRef::new(message_tx, control_tx);

        let result = block_on(async {
            let receive = async {
                let message = message_rx.recv().await.expect("message should arrive");
                let TestMessage::Lookup(value, reply) = message else {
                    panic!("expected Lookup variant");
                };
                assert_eq!(value, 10);
                reply.send(42).expect("reply should be received");
            };

            let call = actor_ref.ask_with(TestMessage::Lookup, 10);
            let ((), result) = futures::future::join(receive, call).await;
            result
        });

        assert_eq!(result.expect("ask_with should succeed"), 42);
    }
}
