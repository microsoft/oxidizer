// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Flume-backed channels with Enactor-owned close and result semantics.
//!
//! The wrapper lets an inbox stop accepting new messages while it still owns
//! its receiver. Actor APIs use crate-owned result types instead of flume types.

use std::marker::PhantomData;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use flume::{Receiver as FlumeReceiver, Sender as FlumeSender};

/// A message rejected because all channel receivers were dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SendError<T>(pub(crate) T);

/// A message not admitted by a non-waiting send.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TrySendFailure<T> {
    /// The bounded channel has no available capacity.
    Full(T),
    /// The channel is closed or disconnected.
    Closed(T),
}

/// The channel closed before another value could be received.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecvError;

/// A non-waiting receive did not return a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TryRecvError {
    /// The channel is open but currently empty.
    Empty,
    /// The channel is closed and contains no buffered values.
    Closed,
}

#[derive(Debug)]
struct ChannelState {
    closed: AtomicBool,
}

impl ChannelState {
    const fn new() -> Self {
        Self {
            closed: AtomicBool::new(false),
        }
    }

    fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
    }

    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }
}

/// Creates typed channel endpoints.
#[derive(Debug)]
pub(crate) struct Channel<T>(PhantomData<fn() -> T>);

/// Sends values through a channel.
#[derive(Debug)]
pub(crate) struct Sender<T> {
    inner: FlumeSender<T>,
    state: Arc<ChannelState>,
}

/// Receives values from a channel.
#[derive(Debug)]
pub(crate) struct Receiver<T> {
    inner: FlumeReceiver<T>,
    state: Arc<ChannelState>,
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            state: Arc::clone(&self.state),
        }
    }
}

impl<T> Sender<T> {
    /// Sends a value after waiting for bounded channel capacity.
    ///
    /// # Errors
    ///
    /// Returns the original value if the channel is closed or disconnected.
    pub(crate) async fn send(&self, message: T) -> Result<(), SendError<T>> {
        if self.state.is_closed() {
            return Err(SendError(message));
        }

        self.inner.send_async(message).await.map_err(|error| SendError(error.0))
    }

    /// Tries to send a value without waiting for channel capacity.
    ///
    /// # Errors
    ///
    /// Returns [`TrySendFailure::Full`] if the bounded channel has no capacity,
    /// or [`TrySendFailure::Closed`] if the channel is closed or disconnected.
    pub(crate) fn try_send(&self, message: T) -> Result<(), TrySendFailure<T>> {
        if self.state.is_closed() {
            return Err(TrySendFailure::Closed(message));
        }

        match self.inner.try_send(message) {
            Ok(()) => Ok(()),
            Err(flume::TrySendError::Full(message)) => Err(TrySendFailure::Full(message)),
            Err(flume::TrySendError::Disconnected(message)) => Err(TrySendFailure::Closed(message)),
        }
    }

    /// Closes the channel to new sends.
    pub(crate) fn close(&self) {
        self.state.close();
    }
}

impl<T> Receiver<T> {
    /// Tries to receive a buffered value without waiting.
    ///
    /// # Errors
    ///
    /// Returns [`TryRecvError::Empty`] if the channel is open but empty, or
    /// [`TryRecvError::Closed`] if it is closed and exhausted.
    pub(crate) fn try_recv(&self) -> Result<T, TryRecvError> {
        match self.inner.try_recv() {
            Ok(message) => Ok(message),
            Err(flume::TryRecvError::Empty) if self.state.is_closed() => Err(TryRecvError::Closed),
            Err(flume::TryRecvError::Empty) => Err(TryRecvError::Empty),
            Err(flume::TryRecvError::Disconnected) => Err(TryRecvError::Closed),
        }
    }

    /// Receives the next value.
    ///
    /// # Errors
    ///
    /// Returns [`RecvError`] if every sender has disconnected.
    pub(crate) async fn recv(&self) -> Result<T, RecvError> {
        match self.inner.recv_async().await {
            Ok(message) => Ok(message),
            Err(flume::RecvError::Disconnected) => Err(RecvError),
        }
    }

    /// Closes the channel to new sends.
    pub(crate) fn close(&self) {
        self.state.close();
    }
}

impl<T> Channel<T> {
    /// Creates a bounded channel.
    pub(crate) fn bounded(capacity: usize) -> (Sender<T>, Receiver<T>) {
        let (tx, rx) = flume::bounded(capacity);
        let state = Arc::new(ChannelState::new());
        (
            Sender {
                inner: tx,
                state: Arc::clone(&state),
            },
            Receiver { inner: rx, state },
        )
    }

    /// Creates an unbounded channel.
    pub(crate) fn unbounded() -> (Sender<T>, Receiver<T>) {
        let (tx, rx) = flume::unbounded();
        let state = Arc::new(ChannelState::new());
        (
            Sender {
                inner: tx,
                state: Arc::clone(&state),
            },
            Receiver { inner: rx, state },
        )
    }
}
