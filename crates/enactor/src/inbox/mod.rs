// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Actor mailboxes and lifecycle control.
//!
//! An [`Inbox`] combines actor messages with a control queue so stop and drain
//! requests can be applied before more messages are delivered.

mod channel;
pub(crate) mod control;
mod delivery;
mod state;

use std::cell::Cell;

use futures::future::{Either, select};
use futures::pin_mut;

use crate::Message;
use crate::exit::LoopExit;

pub(crate) use self::channel::{Channel, Receiver, RecvError, SendError, Sender, TryRecvError, TrySendFailure};
pub(crate) use self::control::{Control, Signal};
pub(crate) use self::delivery::MessageStream;

use self::delivery::Delivery;
use self::state::State;

/// Mailbox for one actor.
///
/// Actor messages and lifecycle requests arrive on separate queues so stop and
/// drain can preempt delivery. An actor written as a receive loop reads its
/// inbox directly. See [`ActorSpawner::spawn_with`](crate::ActorSpawner::spawn_with).
#[derive(Debug)]
pub struct Inbox<M: Message> {
    delivery: Delivery<M>,
    control_rx: Receiver<Control>,
    state: Cell<State>,
}

impl<M: Message> Inbox<M> {
    pub(crate) const fn new(message_rx: Receiver<M>, control_rx: Receiver<Control>, stream: Option<MessageStream<M>>) -> Self {
        Self {
            delivery: Delivery::new(message_rx, stream),
            control_rx,
            state: Cell::new(State::Running),
        }
    }

    /// Receives the next actor message after applying queued control requests.
    ///
    /// # Errors
    ///
    /// Returns [`RecvError`] after a stop, after a drain delivers the last
    /// buffered message, or after the message channel closes and is empty.
    ///
    /// # Panics
    ///
    /// Never panics in practice. The actor context keeps a control sender alive
    /// while the inbox receives, so the control queue cannot close first.
    pub async fn recv(&mut self) -> Result<M, RecvError> {
        loop {
            match self.advance() {
                State::Running => {}
                State::Draining => return self.recv_while_draining(),
                State::Stopped => return Err(RecvError),
            }

            // Receive either a control command or an actual message, prioritizing
            // control commands.
            let control = {
                let control_rx = &self.control_rx;
                let delivery = &mut self.delivery;

                let control_recv = async {
                    control_rx
                        .recv()
                        .await
                        .expect("inbox retains an internal control sender so the control receiver cannot close before drop")
                };
                let message_recv = delivery.recv();
                pin_mut!(control_recv, message_recv);

                match select(control_recv, message_recv).await {
                    Either::Left((control, _)) => control,
                    Either::Right((result, _)) => return result,
                }
            };

            self.transition(control);
        }
    }

    /// Tries to receive the next actor message without waiting.
    ///
    /// # Errors
    ///
    /// Returns [`TryRecvError::Empty`] if no message is ready, or
    /// [`TryRecvError::Closed`] if the inbox stopped or finished draining.
    pub fn try_recv(&self) -> Result<M, TryRecvError> {
        match self.advance() {
            State::Running => self.delivery.try_recv(),
            State::Draining => self.try_recv_while_draining(),
            State::Stopped => Err(TryRecvError::Closed),
        }
    }

    /// Returns how the actor loop ended, from the delivery state.
    ///
    /// A loop that returns while the inbox still runs ended on its own, which
    /// counts as a stop.
    pub(crate) const fn exit(&self) -> LoopExit {
        match self.state() {
            State::Running | State::Stopped => LoopExit::Stopped,
            State::Draining => LoopExit::Drained,
        }
    }

    const fn state(&self) -> State {
        self.state.get()
    }

    fn transition(&self, control: Control) -> State {
        let next = self.state().transition(control);
        self.state.set(next);
        next
    }

    fn try_recv_control(&self) -> Option<Control> {
        self.control_rx.try_recv().ok()
    }

    /// Applies queued control requests and returns the current delivery state.
    fn advance(&self) -> State {
        while let Some(control) = self.try_recv_control() {
            if self.transition(control) == State::Stopped {
                break;
            }
        }

        self.state()
    }

    fn recv_while_draining(&self) -> Result<M, RecvError> {
        match self.delivery.try_recv() {
            Ok(message) => Ok(message),
            Err(TryRecvError::Empty | TryRecvError::Closed) => Err(RecvError),
        }
    }

    fn try_recv_while_draining(&self) -> Result<M, TryRecvError> {
        match self.delivery.try_recv() {
            Ok(message) => Ok(message),
            Err(TryRecvError::Empty | TryRecvError::Closed) => Err(TryRecvError::Closed),
        }
    }

    fn request(&self, signal: Signal) {
        self.delivery.close();
        self.state.set(signal.apply(self.state()));
    }

    /// Closes the message queue, delivers buffered messages, then stops.
    pub(crate) fn drain(&self) {
        self.request(Signal::Drain);
    }

    /// Closes the message queue and stops delivery immediately.
    pub(crate) fn stop(&self) {
        self.request(Signal::Stop);
    }

    /// Ends delivery after a receive-loop body returns, and reports how the loop ended.
    ///
    /// Buffered messages are dropped now. Otherwise they would stay in the
    /// channel until every sender drops, and an ask that waits for one of them
    /// would wait forever. A drain that left messages unread did not finish, so
    /// it counts as a stop.
    pub(crate) fn finish(&self) -> LoopExit {
        self.delivery.close();
        let unread = std::iter::from_fn(|| self.delivery.try_recv().ok()).count();
        match (self.exit(), unread) {
            (LoopExit::Drained, 0) => LoopExit::Drained,
            _ => LoopExit::Stopped,
        }
    }
}

impl<M: Message> Drop for Inbox<M> {
    fn drop(&mut self) {
        self.delivery.close();
        self.control_rx.close();
    }
}

#[cfg(test)]
mod tests {
    use std::pin::pin;
    use std::task::{Context as TaskContext, Poll, Waker};

    use super::{Channel, Control, Inbox, Signal, TryRecvError};

    #[test]
    fn recv_observes_stop_while_waiting() {
        let (_message_tx, message_rx) = Channel::<u8>::bounded(1);
        let (control_tx, control_rx) = Channel::unbounded();
        let mut inbox = Inbox::new(message_rx, control_rx, None);

        let mut recv = pin!(inbox.recv());
        let waker = Waker::noop();
        let mut cx = TaskContext::from_waker(waker);

        assert!(matches!(recv.as_mut().poll(&mut cx), Poll::Pending));

        control_tx
            .try_send(Control::Signal(Signal::Stop))
            .expect("stop control should queue while recv waits");

        assert!(matches!(recv.as_mut().poll(&mut cx), Poll::Ready(Err(_))));
    }

    #[test]
    fn recv_prioritizes_stop_over_ready_message() {
        let (message_tx, message_rx) = Channel::<u8>::bounded(1);
        let (control_tx, control_rx) = Channel::unbounded();
        let mut inbox = Inbox::new(message_rx, control_rx, None);

        let mut recv = pin!(inbox.recv());
        let waker = Waker::noop();
        let mut cx = TaskContext::from_waker(waker);

        assert!(matches!(recv.as_mut().poll(&mut cx), Poll::Pending));

        message_tx.try_send(7).expect("message should queue while recv waits");
        control_tx
            .try_send(Control::Signal(Signal::Stop))
            .expect("stop control should queue while recv waits");

        assert!(matches!(recv.as_mut().poll(&mut cx), Poll::Ready(Err(_))));
    }

    #[test]
    fn recv_uses_latest_pending_lifecycle() {
        let (message_tx, message_rx) = Channel::<u8>::bounded(1);
        let (control_tx, control_rx) = Channel::unbounded();
        let inbox = Inbox::new(message_rx, control_rx, None);

        message_tx.try_send(7).expect("message should queue before controls are observed");

        control_tx.try_send(Control::Signal(Signal::Drain)).expect("drain should enqueue");
        control_tx
            .try_send(Control::Signal(Signal::Stop))
            .expect("stop upgrade should enqueue");

        assert!(matches!(inbox.try_recv(), Err(TryRecvError::Closed)));
    }

    #[test]
    fn spammy_drain_requests_still_process_buffered_messages() {
        let (message_tx, message_rx) = Channel::<u8>::bounded(2);
        let (control_tx, control_rx) = Channel::unbounded();
        let inbox = Inbox::new(message_rx, control_rx, None);

        message_tx.try_send(7).expect("first buffered message should queue");
        message_tx.try_send(8).expect("second buffered message should queue");

        for _ in 0..32 {
            control_tx
                .try_send(Control::Signal(Signal::Drain))
                .expect("drain requests should enqueue");
        }

        assert_eq!(inbox.try_recv(), Ok(7));
        assert_eq!(inbox.try_recv(), Ok(8));
        assert!(matches!(inbox.try_recv(), Err(TryRecvError::Closed)));
    }
}
