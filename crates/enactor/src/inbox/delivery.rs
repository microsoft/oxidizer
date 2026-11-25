// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Message-side delivery source for an [`Inbox`](super::Inbox).
//!
//! An inbox pulls actor messages from a [`Delivery`]: a channel, optionally
//! merged with a stream installed at spawn time.

use std::pin::Pin;

use futures::future::{Either, select};
use futures::{Stream, StreamExt, pin_mut};

use super::channel::{Receiver, RecvError, TryRecvError};
use crate::Message;

pub(crate) type MessageStream<M> = Pin<Box<dyn Stream<Item = M> + Send>>;

pub(crate) struct Delivery<M: Message> {
    channel: Receiver<M>,
    stream: Option<MessageStream<M>>,
}

impl<M: Message> Delivery<M> {
    /// Builds a delivery source from a channel and an optional merged stream.
    pub(crate) const fn new(channel: Receiver<M>, stream: Option<MessageStream<M>>) -> Self {
        Self { channel, stream }
    }

    /// Waits for the next message, returning `Err(RecvError)` once the channel
    /// is closed and exhausted.
    ///
    /// When a stream is merged, buffered channel messages are preferred so
    /// addressed work is never starved by a hot stream.
    pub(crate) async fn recv(&mut self) -> Result<M, RecvError> {
        if let Some(stream) = self.stream.as_mut() {
            match self.channel.try_recv() {
                Ok(message) => return Ok(message),
                Err(TryRecvError::Closed) => return Err(RecvError),
                Err(TryRecvError::Empty) => {}
            }

            let mailbox_recv = self.channel.recv();
            let stream_next = stream.next();
            pin_mut!(mailbox_recv, stream_next);

            match select(mailbox_recv, stream_next).await {
                Either::Left((Ok(message), _)) | Either::Right((Some(message), _)) => {
                    return Ok(message);
                }
                Either::Left((Err(RecvError), _)) => return Err(RecvError),
                Either::Right((None, _)) => self.stream = None,
            }
        }

        self.channel.recv().await
    }

    /// Returns a buffered channel message without waiting.
    pub(crate) fn try_recv(&self) -> Result<M, TryRecvError> {
        self.channel.try_recv()
    }

    /// Closes the channel so further sends fail.
    pub(crate) fn close(&self) {
        self.channel.close();
    }
}

impl<M: Message> std::fmt::Debug for Delivery<M> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Delivery")
            .field("has_stream", &self.stream.is_some())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use std::pin::pin;
    use std::task::{Context as TaskContext, Poll, Waker};

    use futures::executor::block_on;
    use futures::stream;

    use super::{Delivery, MessageStream};
    use crate::inbox::{Channel, RecvError};

    fn stream_of(items: &[u8]) -> MessageStream<u8> {
        Box::pin(stream::iter(items.to_vec()))
    }

    #[test]
    fn recv_without_stream_delivers_channel_messages() {
        let (tx, rx) = Channel::<u8>::bounded(4);
        let mut delivery = Delivery::new(rx, None);
        tx.try_send(7).expect("message should queue");

        assert_eq!(block_on(delivery.recv()), Ok(7));
    }

    #[test]
    fn recv_without_stream_errors_when_channel_closed() {
        let (tx, rx) = Channel::<u8>::bounded(1);
        let mut delivery = Delivery::new(rx, None);
        drop(tx);

        assert_eq!(block_on(delivery.recv()), Err(RecvError));
    }

    #[test]
    fn recv_prefers_buffered_channel_message_over_stream() {
        let (tx, rx) = Channel::<u8>::bounded(4);
        tx.try_send(1).expect("message should queue");
        let mut delivery = Delivery::new(rx, Some(stream_of(&[10, 20])));

        assert_eq!(block_on(delivery.recv()), Ok(1));
    }

    #[test]
    fn recv_delivers_stream_item_when_channel_empty() {
        // Keep the sender alive so the channel stays open but empty.
        let (_tx, rx) = Channel::<u8>::bounded(4);
        let mut delivery = Delivery::new(rx, Some(stream_of(&[10])));

        assert_eq!(block_on(delivery.recv()), Ok(10));
    }

    #[test]
    fn recv_errors_when_channel_closes_even_with_live_stream() {
        let (tx, rx) = Channel::<u8>::bounded(1);
        let mut delivery = Delivery::new(rx, Some(stream_of(&[10, 20])));
        drop(tx);

        // A closed mailbox stops the actor even though the stream has items.
        assert_eq!(block_on(delivery.recv()), Err(RecvError));
    }

    #[test]
    fn recv_retires_ended_stream_then_delivers_from_channel() {
        let (tx, rx) = Channel::<u8>::bounded(4);
        let mut delivery = Delivery::new(rx, Some(stream_of(&[])));

        let waker = Waker::noop();
        let mut cx = TaskContext::from_waker(waker);

        {
            let mut recv = pin!(delivery.recv());
            assert!(matches!(recv.as_mut().poll(&mut cx), Poll::Pending));
        }

        tx.try_send(42).expect("message should queue");
        assert_eq!(block_on(delivery.recv()), Ok(42));
    }

    #[test]
    fn try_recv_reads_only_the_channel() {
        let (tx, rx) = Channel::<u8>::bounded(4);
        let delivery = Delivery::new(rx, Some(stream_of(&[10])));

        delivery.try_recv().unwrap_err();

        tx.try_send(5).expect("message should queue");
        assert_eq!(delivery.try_recv(), Ok(5));
    }
}
