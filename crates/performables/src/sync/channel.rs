// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Channels with optional telemetry.

use std::collections::VecDeque;
use std::error::Error as StdError;
use std::fmt;
use std::ops::Deref;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex, MutexGuard as StdMutexGuard, PoisonError};
use std::task::{Context, Poll};
use std::time::Duration;

use super::wait_queue::{WaitQueue, Waiter, block_on, block_on_timeout};
use crate::telemetry::{self, EventKind};

const ERROR_FULL: u8 = 1;
const ERROR_EMPTY: u8 = 2;
const ERROR_CLOSED: u8 = 3;
const ERROR_TIMEOUT: u8 = 4;

/// An opaque channel operation error.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct Error<T = ()> {
    code: u8,
    value: Option<T>,
}

impl<T> Error<T> {
    fn with_value(code: u8, value: T) -> Self {
        Self { code, value: Some(value) }
    }

    /// Returns whether a bounded channel had no available capacity.
    #[must_use]
    pub const fn is_full(&self) -> bool {
        self.code == ERROR_FULL
    }

    /// Returns whether a channel had no value available without waiting.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.code == ERROR_EMPTY
    }

    /// Returns whether the channel no longer accepts the operation.
    #[must_use]
    pub const fn is_closed(&self) -> bool {
        self.code == ERROR_CLOSED
    }

    /// Returns whether a blocking operation reached its deadline.
    #[must_use]
    pub const fn is_timeout(&self) -> bool {
        self.code == ERROR_TIMEOUT
    }

    /// Returns the value rejected by a send operation, if present.
    #[must_use]
    pub fn into_value(self) -> Option<T> {
        self.value
    }
}

impl Error {
    const fn without_value(code: u8) -> Self {
        Self { code, value: None }
    }
}

impl<T> fmt::Display for Error<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self.code {
            ERROR_FULL => "channel has no available capacity",
            ERROR_EMPTY => "channel has no value available",
            ERROR_CLOSED => "channel is closed",
            ERROR_TIMEOUT => "channel operation timed out",
            _ => "channel operation failed",
        })
    }
}

impl<T> fmt::Debug for Error<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Error").finish_non_exhaustive()
    }
}

impl<T: 'static> StdError for Error<T> {}

/// Creates a bounded multi-producer, multi-consumer queue channel.
///
/// # Panics
///
/// Panics if `capacity` is zero.
#[must_use]
pub fn bounded<T>(capacity: usize) -> (Sender<T>, Receiver<T>) {
    assert!(capacity > 0, "bounded channel capacity must be nonzero");
    queue(Some(capacity))
}

/// Creates an unbounded multi-producer, multi-consumer queue channel.
#[must_use]
pub fn unbounded<T>() -> (Sender<T>, Receiver<T>) {
    queue(None)
}

fn queue<T>(capacity: Option<usize>) -> (Sender<T>, Receiver<T>) {
    let values = capacity.map_or_else(VecDeque::new, VecDeque::with_capacity);
    let shared = Arc::new(QueueShared {
        state: StdMutex::new(QueueState {
            values,
            capacity,
            senders: 1,
            receivers: 1,
            closed: false,
            high_watermark: 0,
        }),
        send_waiters: WaitQueue::new(),
        receive_waiters: WaitQueue::new(),
    });
    (
        Sender {
            shared: Arc::clone(&shared),
        },
        Receiver { shared },
    )
}

#[derive(Debug)]
struct QueueState<T> {
    values: VecDeque<T>,
    capacity: Option<usize>,
    senders: usize,
    receivers: usize,
    closed: bool,
    high_watermark: usize,
}

impl<T> QueueState<T> {
    fn send_is_closed(&self) -> bool {
        self.closed || self.receivers == 0
    }

    fn receive_is_closed(&self) -> bool {
        self.closed || self.senders == 0
    }

    fn has_send_capacity(&self) -> bool {
        self.capacity.is_none_or(|capacity| self.values.len() < capacity)
    }
}

#[derive(Debug)]
struct QueueShared<T> {
    state: StdMutex<QueueState<T>>,
    send_waiters: WaitQueue,
    receive_waiters: WaitQueue,
}

impl<T> QueueShared<T> {
    fn state(&self) -> StdMutexGuard<'_, QueueState<T>> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn record(&self, kind: EventKind) {
        telemetry::record(kind, std::ptr::from_ref(self).cast::<()>());
    }

    fn try_send(&self, value: T) -> Result<(), (u8, T)> {
        let mut state = self.state();
        if state.send_is_closed() {
            return Err((ERROR_CLOSED, value));
        }
        if !state.has_send_capacity() {
            return Err((ERROR_FULL, value));
        }
        state.values.push_back(value);
        let len = state.values.len();
        let new_high_watermark = (len > state.high_watermark).then(|| {
            state.high_watermark = len;
            len
        });
        drop(state);
        self.record(EventKind::ChannelSend);
        if let Some(high_watermark) = new_high_watermark {
            telemetry::record_channel_high_watermark(std::ptr::from_ref(self).cast::<()>(), high_watermark);
        }
        self.receive_waiters.wake_one();
        Ok(())
    }

    fn try_receive(&self) -> Result<T, Error> {
        let mut state = self.state();
        if let Some(value) = state.values.pop_front() {
            drop(state);
            self.record(EventKind::ChannelReceive);
            self.send_waiters.wake_one();
            return Ok(value);
        }
        if state.receive_is_closed() {
            Err(Error::without_value(ERROR_CLOSED))
        } else {
            Err(Error::without_value(ERROR_EMPTY))
        }
    }

    fn send_wait_complete(&self) -> bool {
        let state = self.state();
        state.send_is_closed() || state.has_send_capacity()
    }

    fn receive_wait_complete(&self) -> bool {
        let state = self.state();
        !state.values.is_empty() || state.receive_is_closed()
    }

    fn close(&self) -> bool {
        let closed = {
            let mut state = self.state();
            if state.closed {
                false
            } else {
                state.closed = true;
                true
            }
        };
        if closed {
            self.record(EventKind::ChannelClose);
            self.send_waiters.wake_all_marked(|| {});
            self.receive_waiters.wake_all_marked(|| {});
        }
        closed
    }
}

/// Sending endpoint of a queue channel.
pub struct Sender<T> {
    shared: Arc<QueueShared<T>>,
}

impl<T> Sender<T> {
    /// Sends `value`, waiting asynchronously while a bounded channel is full.
    ///
    /// Cancelling the returned future before completion leaves the value
    /// unsent and does not consume channel capacity.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] with `value` if all receivers are gone or the
    /// channel was closed.
    pub async fn send(&self, value: T) -> Result<(), Error<T>> {
        let mut value = value;
        let mut contention_recorded = false;
        loop {
            match self.shared.try_send(value) {
                Ok(()) => return Ok(()),
                Err((ERROR_CLOSED, value)) => return Err(Error::with_value(ERROR_CLOSED, value)),
                Err((_code, returned)) => {
                    value = returned;
                    if contention_recorded {
                        QueueWait::send(&self.shared).await;
                        continue;
                    }
                    self.shared.record(EventKind::ChannelSendContention);
                    contention_recorded = true;
                    QueueWait::send(&self.shared).await;
                }
            }
        }
    }

    /// Sends `value`, blocking the current thread while a bounded channel is full.
    ///
    /// This method must not run on an executor thread required to make progress
    /// on a task receiving from this channel.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] with `value` if all receivers are gone or the
    /// channel was closed.
    pub fn send_sync(&self, value: T) -> Result<(), Error<T>> {
        block_on(self.send(value))
    }

    /// Attempts to send `value` without waiting.
    ///
    /// # Errors
    ///
    /// Returns an error when a bounded channel has no capacity or receivers no
    /// longer accept values.
    pub fn try_send(&self, value: T) -> Result<(), Error<T>> {
        let result = self.shared.try_send(value).map_err(|(code, value)| Error::with_value(code, value));
        if result.as_ref().is_err_and(Error::is_full) {
            self.shared.record(EventKind::ChannelSendContention);
        }
        result
    }

    /// Prevents future sends and wakes blocked channel operations.
    ///
    /// Values already buffered remain available to receivers. Returns `true`
    /// when this call closes the channel.
    #[must_use]
    pub fn close(&self) -> bool {
        self.shared.close()
    }

    /// Returns whether receivers no longer accept new values.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.shared.state().send_is_closed()
    }

    /// Returns the number of buffered values.
    #[must_use]
    pub fn len(&self) -> usize {
        self.shared.state().values.len()
    }

    /// Returns whether the channel contains no buffered values.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.shared.state().values.is_empty()
    }

    /// Returns the configured bound, or `None` for an unbounded channel.
    #[must_use]
    pub fn capacity(&self) -> Option<usize> {
        self.shared.state().capacity
    }
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        self.shared.state().senders += 1;
        Self {
            shared: Arc::clone(&self.shared),
        }
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        let final_sender = {
            let mut state = self.shared.state();
            state.senders -= 1;
            state.senders == 0
        };
        if final_sender {
            self.shared.record(EventKind::ChannelClose);
            self.shared.receive_waiters.wake_all_marked(|| {});
        }
    }
}

impl<T> fmt::Debug for Sender<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = self.shared.state();
        f.debug_struct("Sender")
            .field("len", &state.values.len())
            .field("capacity", &state.capacity)
            .field("closed", &state.send_is_closed())
            .finish_non_exhaustive()
    }
}

/// Receiving endpoint of a queue channel.
pub struct Receiver<T> {
    shared: Arc<QueueShared<T>>,
}

impl<T> Receiver<T> {
    /// Receives the next value, waiting asynchronously while the channel is empty.
    ///
    /// Buffered values remain available after the final sender is dropped.
    /// Cancelling the returned future does not consume a value.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] after the channel closes and all buffered values
    /// have been received.
    pub async fn recv(&self) -> Result<T, Error> {
        let mut contention_recorded = false;
        loop {
            match self.shared.try_receive() {
                Ok(value) => return Ok(value),
                Err(error) =>
                {
                    #[expect(clippy::single_match_else, reason = "closed is terminal while every other code retries")]
                    match error.code {
                        ERROR_CLOSED => return Err(error),
                        _ => {
                            if contention_recorded {
                                QueueWait::receive(&self.shared).await;
                                continue;
                            }
                            self.shared.record(EventKind::ChannelReceiveContention);
                            contention_recorded = true;
                            QueueWait::receive(&self.shared).await;
                        }
                    }
                }
            }
        }
    }

    /// Receives the next value, blocking the current thread while the channel is empty.
    ///
    /// This method must not run on an executor thread required to make progress
    /// on a task sending to this channel.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] after the channel closes and all buffered values
    /// have been received.
    pub fn recv_sync(&self) -> Result<T, Error> {
        block_on(self.recv())
    }

    /// Receives the next value, blocking for at most `timeout`.
    ///
    /// # Errors
    ///
    /// Returns an error if the deadline expires or the channel closes while
    /// drained.
    pub fn recv_timeout_sync(&self, timeout: Duration) -> Result<T, Error> {
        match block_on_timeout(self.recv(), timeout) {
            Some(Ok(value)) => Ok(value),
            Some(Err(error)) => Err(error),
            None => Err(Error::without_value(ERROR_TIMEOUT)),
        }
    }

    /// Attempts to receive the next value without waiting.
    ///
    /// # Errors
    ///
    /// Returns an error when no value is currently buffered.
    pub fn try_recv(&self) -> Result<T, Error> {
        let result = self.shared.try_receive();
        if result.as_ref().is_err_and(Error::is_empty) {
            self.shared.record(EventKind::ChannelReceiveContention);
        }
        result
    }

    /// Prevents future sends and wakes blocked senders.
    ///
    /// Values already buffered remain available to all receiver clones.
    /// Returns `true` when this call closes the channel.
    #[must_use]
    pub fn close(&self) -> bool {
        self.shared.close()
    }

    /// Returns whether no additional values can be sent.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.shared.state().receive_is_closed()
    }

    /// Returns the number of buffered values.
    #[must_use]
    pub fn len(&self) -> usize {
        self.shared.state().values.len()
    }

    /// Returns whether the channel contains no buffered values.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.shared.state().values.is_empty()
    }

    /// Returns the configured bound, or `None` for an unbounded channel.
    #[must_use]
    pub fn capacity(&self) -> Option<usize> {
        self.shared.state().capacity
    }
}

impl<T> Clone for Receiver<T> {
    fn clone(&self) -> Self {
        self.shared.state().receivers += 1;
        Self {
            shared: Arc::clone(&self.shared),
        }
    }
}

impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        let (final_receiver, buffered) = {
            let mut state = self.shared.state();
            state.receivers -= 1;
            if state.receivers == 0 {
                state.closed = true;
                (true, Some(std::mem::take(&mut state.values)))
            } else {
                (false, None)
            }
        };
        if final_receiver {
            self.shared.record(EventKind::ChannelClose);
            self.shared.send_waiters.wake_all_marked(|| {});
            drop(buffered);
        }
    }
}

impl<T> fmt::Debug for Receiver<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = self.shared.state();
        f.debug_struct("Receiver")
            .field("len", &state.values.len())
            .field("capacity", &state.capacity)
            .field("closed", &state.receive_is_closed())
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug)]
enum QueueWaitKind {
    Send,
    Receive,
}

#[derive(Debug)]
struct QueueWait<'a, T> {
    shared: &'a QueueShared<T>,
    kind: QueueWaitKind,
    waiter: Option<Arc<Waiter>>,
}

impl<'a, T> QueueWait<'a, T> {
    fn send(shared: &'a QueueShared<T>) -> Self {
        Self {
            shared,
            kind: QueueWaitKind::Send,
            waiter: None,
        }
    }

    fn receive(shared: &'a QueueShared<T>) -> Self {
        Self {
            shared,
            kind: QueueWaitKind::Receive,
            waiter: None,
        }
    }

    fn complete(&self) -> bool {
        match self.kind {
            QueueWaitKind::Send => self.shared.send_wait_complete(),
            QueueWaitKind::Receive => self.shared.receive_wait_complete(),
        }
    }

    fn waiters(&self) -> &WaitQueue {
        match self.kind {
            QueueWaitKind::Send => &self.shared.send_waiters,
            QueueWaitKind::Receive => &self.shared.receive_waiters,
        }
    }

    fn poll_registered(&mut self, cx: &Context<'_>) -> Poll<()> {
        let waiter = Arc::clone(self.waiter.get_or_insert_with(|| Arc::new(Waiter::new())));
        waiter.register(cx.waker());
        if self.waiters().enqueue_if_needed(&waiter, || self.complete()) {
            self.waiter.take();
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }
}

impl<T> Future for QueueWait<'_, T> {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.complete() {
            return Poll::Ready(());
        }
        self.poll_registered(cx)
    }
}

impl<T> Drop for QueueWait<'_, T> {
    fn drop(&mut self) {
        if let Some(waiter) = &self.waiter {
            let removed = self.waiters().cancel(waiter);
            if !removed && self.complete() {
                self.waiters().wake_one();
            }
        }
    }
}

/// Creates a channel that transfers at most one value.
pub fn oneshot<T>() -> (OneshotSender<T>, OneshotReceiver<T>) {
    let shared = Arc::new(OneshotShared {
        state: StdMutex::new(OneshotState {
            value: None,
            sender_alive: true,
            receiver_alive: true,
        }),
        receiver_waiters: WaitQueue::new(),
    });
    (
        OneshotSender {
            shared: Arc::clone(&shared),
            active: true,
        },
        OneshotReceiver {
            shared,
            waiter: None,
            active: true,
            contention_recorded: false,
        },
    )
}

#[derive(Debug)]
struct OneshotState<T> {
    value: Option<T>,
    sender_alive: bool,
    receiver_alive: bool,
}

#[derive(Debug)]
struct OneshotShared<T> {
    state: StdMutex<OneshotState<T>>,
    receiver_waiters: WaitQueue,
}

impl<T> OneshotShared<T> {
    fn state(&self) -> StdMutexGuard<'_, OneshotState<T>> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn receiver_ready(&self) -> bool {
        let state = self.state();
        state.value.is_some() || !state.sender_alive
    }
}

/// Sending endpoint of a oneshot channel.
pub struct OneshotSender<T> {
    shared: Arc<OneshotShared<T>>,
    active: bool,
}

impl<T> OneshotSender<T> {
    /// Sends the channel's value, returning it if the receiver was cancelled.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] with `value` if the receiver was dropped.
    pub fn send(mut self, value: T) -> Result<(), Error<T>> {
        let mut state = self.shared.state();
        self.active = false;
        state.sender_alive = false;
        if !state.receiver_alive {
            return Err(Error::with_value(ERROR_CLOSED, value));
        }
        state.value = Some(value);
        drop(state);
        telemetry::record(EventKind::ChannelSend, std::ptr::from_ref(&*self.shared).cast::<()>());
        self.shared.receiver_waiters.wake_all_marked(|| {});
        Ok(())
    }

    /// Returns whether the receiver has been dropped.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        !self.shared.state().receiver_alive
    }
}

impl<T> Drop for OneshotSender<T> {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        self.shared.state().sender_alive = false;
        telemetry::record(EventKind::ChannelClose, std::ptr::from_ref(&*self.shared).cast::<()>());
        self.shared.receiver_waiters.wake_all_marked(|| {});
    }
}

impl<T> fmt::Debug for OneshotSender<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OneshotSender")
            .field("closed", &self.is_closed())
            .finish_non_exhaustive()
    }
}

/// Receiving endpoint and future of a oneshot channel.
///
/// The receiver can be awaited directly. Dropping it cancels the channel and
/// causes [`OneshotSender::send`] to return the unsent value.
#[must_use = "dropping the receiver cancels the oneshot channel"]
pub struct OneshotReceiver<T> {
    shared: Arc<OneshotShared<T>>,
    waiter: Option<Arc<Waiter>>,
    active: bool,
    contention_recorded: bool,
}

impl<T> OneshotReceiver<T> {
    /// Blocks the current thread until the value arrives or the sender is dropped.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if the sender is dropped without sending.
    pub fn recv_sync(mut self) -> Result<T, Error> {
        block_on(&mut self)
    }

    /// Blocks for at most `timeout` until the value arrives.
    ///
    /// # Errors
    ///
    /// Returns an error if the deadline expires or the sender drops without
    /// sending.
    pub fn recv_timeout_sync(mut self, timeout: Duration) -> Result<T, Error> {
        match block_on_timeout(&mut self, timeout) {
            Some(Ok(value)) => Ok(value),
            Some(Err(error)) => Err(error),
            None => Err(Error::without_value(ERROR_TIMEOUT)),
        }
    }

    /// Attempts to receive the value without waiting.
    ///
    /// # Errors
    ///
    /// Returns an error before the sender completes or if it drops without
    /// sending.
    pub fn try_recv(&mut self) -> Result<T, Error> {
        match self.take_result() {
            Some(Ok(value)) => Ok(value),
            Some(Err(error)) => Err(error),
            None => {
                if self.contention_recorded {
                    return Err(Error::without_value(ERROR_EMPTY));
                }
                telemetry::record(EventKind::ChannelReceiveContention, std::ptr::from_ref(&*self.shared).cast::<()>());
                self.contention_recorded = true;
                Err(Error::without_value(ERROR_EMPTY))
            }
        }
    }

    /// Returns whether the sender was dropped without sending.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        let state = self.shared.state();
        !state.sender_alive && state.value.is_none()
    }

    fn take_result(&mut self) -> Option<Result<T, Error>> {
        let mut state = self.shared.state();
        if let Some(value) = state.value.take() {
            self.active = false;
            state.receiver_alive = false;
            drop(state);
            if let Some(waiter) = self.waiter.take() {
                self.shared.receiver_waiters.cancel(&waiter);
            }
            telemetry::record(EventKind::ChannelReceive, std::ptr::from_ref(&*self.shared).cast::<()>());
            telemetry::record(EventKind::ChannelClose, std::ptr::from_ref(&*self.shared).cast::<()>());
            return Some(Ok(value));
        }
        if !state.sender_alive {
            self.active = false;
            state.receiver_alive = false;
            drop(state);
            if let Some(waiter) = self.waiter.take() {
                self.shared.receiver_waiters.cancel(&waiter);
            }
            return Some(Err(Error::without_value(ERROR_CLOSED)));
        }
        None
    }
}

impl<T> Future for OneshotReceiver<T> {
    type Output = Result<T, Error>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if let Some(result) = self.take_result() {
            return Poll::Ready(result);
        }
        if self.contention_recorded {
            return self.poll_waiter(cx);
        }
        telemetry::record(EventKind::ChannelReceiveContention, std::ptr::from_ref(&*self.shared).cast::<()>());
        self.contention_recorded = true;
        self.poll_waiter(cx)
    }
}

impl<T> OneshotReceiver<T> {
    fn poll_waiter(&mut self, cx: &Context<'_>) -> Poll<Result<T, Error>> {
        let waiter = Arc::clone(self.waiter.get_or_insert_with(|| Arc::new(Waiter::new())));
        waiter.register(cx.waker());
        if self
            .shared
            .receiver_waiters
            .enqueue_if_needed(&waiter, || self.shared.receiver_ready())
        {
            self.waiter.take();
            self.take_result().map_or(Poll::Pending, Poll::Ready)
        } else {
            Poll::Pending
        }
    }
}

impl<T> Drop for OneshotReceiver<T> {
    fn drop(&mut self) {
        if let Some(waiter) = &self.waiter {
            self.shared.receiver_waiters.cancel(waiter);
        }
        if !self.active {
            return;
        }
        let value = {
            let mut state = self.shared.state();
            state.receiver_alive = false;
            state.value.take()
        };
        telemetry::record(EventKind::ChannelClose, std::ptr::from_ref(&*self.shared).cast::<()>());
        drop(value);
    }
}

impl<T> fmt::Debug for OneshotReceiver<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OneshotReceiver")
            .field("closed", &self.is_closed())
            .finish_non_exhaustive()
    }
}

/// Creates a latest-value channel initialized with `initial`.
#[must_use]
pub fn watch<T>(initial: T) -> (WatchSender<T>, WatchReceiver<T>) {
    let shared = Arc::new(WatchShared {
        state: StdMutex::new(WatchState {
            value: initial,
            version: 0,
            senders: 1,
            receivers: 1,
        }),
        receiver_waiters: WaitQueue::new(),
    });
    (
        WatchSender {
            shared: Arc::clone(&shared),
        },
        WatchReceiver {
            shared,
            observed: AtomicU64::new(0),
        },
    )
}

#[derive(Debug)]
struct WatchState<T> {
    value: T,
    version: u64,
    senders: usize,
    receivers: usize,
}

#[derive(Debug)]
struct WatchShared<T> {
    state: StdMutex<WatchState<T>>,
    receiver_waiters: WaitQueue,
}

impl<T> WatchShared<T> {
    fn state(&self) -> StdMutexGuard<'_, WatchState<T>> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Sending endpoint of a latest-value watch channel.
pub struct WatchSender<T> {
    shared: Arc<WatchShared<T>>,
}

impl<T> WatchSender<T> {
    /// Replaces the latest value, returning it unchanged if there are no receivers.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] with `value` when no receivers remain.
    pub fn send(&self, value: T) -> Result<(), Error<T>> {
        let mut state = self.shared.state();
        if state.receivers == 0 {
            return Err(Error::with_value(ERROR_CLOSED, value));
        }
        let previous = std::mem::replace(&mut state.value, value);
        state.version = state.version.wrapping_add(1);
        drop(state);
        telemetry::record(EventKind::ChannelSend, std::ptr::from_ref(&*self.shared).cast::<()>());
        self.shared.receiver_waiters.wake_all_marked(|| {});
        drop(previous);
        Ok(())
    }

    /// Replaces and returns the previous value even when there are no receivers.
    pub fn send_replace(&self, value: T) -> T {
        let mut state = self.shared.state();
        let previous = std::mem::replace(&mut state.value, value);
        state.version = state.version.wrapping_add(1);
        drop(state);
        telemetry::record(EventKind::ChannelSend, std::ptr::from_ref(&*self.shared).cast::<()>());
        self.shared.receiver_waiters.wake_all_marked(|| {});
        previous
    }

    /// Modifies the latest value and notifies receivers.
    ///
    /// The closure executes while the value is locked and must not re-enter
    /// operations on this channel.
    pub fn send_modify(&self, modify: impl FnOnce(&mut T)) {
        let mut state = self.shared.state();
        modify(&mut state.value);
        state.version = state.version.wrapping_add(1);
        drop(state);
        telemetry::record(EventKind::ChannelSend, std::ptr::from_ref(&*self.shared).cast::<()>());
        self.shared.receiver_waiters.wake_all_marked(|| {});
    }

    /// Borrows the latest value.
    ///
    /// Holding the returned guard blocks sender updates.
    #[must_use]
    pub fn borrow(&self) -> WatchRef<'_, T> {
        telemetry::record(EventKind::ChannelReceive, std::ptr::from_ref(&*self.shared).cast::<()>());
        WatchRef {
            guard: self.shared.state(),
        }
    }

    /// Creates a receiver that initially considers the current value observed.
    #[must_use]
    pub fn subscribe(&self) -> WatchReceiver<T> {
        let version = {
            let mut state = self.shared.state();
            state.receivers += 1;
            state.version
        };
        WatchReceiver {
            shared: Arc::clone(&self.shared),
            observed: AtomicU64::new(version),
        }
    }

    /// Returns whether all receivers have been dropped.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.shared.state().receivers == 0
    }
}

impl<T> Clone for WatchSender<T> {
    fn clone(&self) -> Self {
        self.shared.state().senders += 1;
        Self {
            shared: Arc::clone(&self.shared),
        }
    }
}

impl<T> Drop for WatchSender<T> {
    fn drop(&mut self) {
        let final_sender = {
            let mut state = self.shared.state();
            state.senders -= 1;
            state.senders == 0
        };
        if final_sender {
            telemetry::record(EventKind::ChannelClose, std::ptr::from_ref(&*self.shared).cast::<()>());
            self.shared.receiver_waiters.wake_all_marked(|| {});
        }
    }
}

impl<T> fmt::Debug for WatchSender<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WatchSender")
            .field("closed", &self.is_closed())
            .finish_non_exhaustive()
    }
}

/// Receiving endpoint of a latest-value watch channel.
///
/// Each receiver clone maintains an independent observed version.
pub struct WatchReceiver<T> {
    shared: Arc<WatchShared<T>>,
    observed: AtomicU64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WatchChange {
    Version(u64),
    Closed,
    Pending,
}

impl<T> WatchReceiver<T> {
    /// Borrows the latest value without marking its version observed.
    ///
    /// Holding the returned guard blocks sender updates.
    pub fn borrow(&self) -> WatchRef<'_, T> {
        telemetry::record(EventKind::ChannelReceive, std::ptr::from_ref(&*self.shared).cast::<()>());
        WatchRef {
            guard: self.shared.state(),
        }
    }

    /// Borrows the latest value and marks its version observed.
    ///
    /// Holding the returned guard blocks sender updates.
    pub fn borrow_and_update(&self) -> WatchRef<'_, T> {
        let guard = self.shared.state();
        self.observed.store(guard.version, Ordering::Release);
        telemetry::record(EventKind::ChannelReceive, std::ptr::from_ref(&*self.shared).cast::<()>());
        WatchRef { guard }
    }

    /// Returns whether a newer version is available.
    ///
    /// A final unobserved value is reported before channel closure.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when all senders are gone and no newer
    /// version remains unobserved.
    pub fn has_changed(&self) -> Result<bool, Error> {
        match self.change_status() {
            WatchChange::Version(_) => Ok(true),
            WatchChange::Closed => Err(Error::without_value(ERROR_CLOSED)),
            WatchChange::Pending => Ok(false),
        }
    }

    /// Waits asynchronously for a newer version and marks it observed.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when all senders are gone and no newer
    /// version remains unobserved.
    pub async fn changed(&self) -> Result<(), Error> {
        let mut contention_recorded = false;
        loop {
            match self.observe_change() {
                WatchChange::Version(_) => {
                    telemetry::record(EventKind::ChannelReceive, std::ptr::from_ref(&*self.shared).cast::<()>());
                    return Ok(());
                }
                WatchChange::Closed => return Err(Error::without_value(ERROR_CLOSED)),
                WatchChange::Pending => {}
            }
            if contention_recorded {
                WatchChanged {
                    receiver: self,
                    waiter: None,
                }
                .await;
                continue;
            }
            telemetry::record(EventKind::ChannelReceiveContention, std::ptr::from_ref(&*self.shared).cast::<()>());
            contention_recorded = true;
            WatchChanged {
                receiver: self,
                waiter: None,
            }
            .await;
        }
    }

    /// Waits until `predicate` accepts the latest value and returns it borrowed.
    ///
    /// Every value examined by the predicate is considered observed. The
    /// predicate executes while the value is locked and must not re-enter
    /// operations on this channel. Holding the returned guard blocks updates.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when all senders are gone and the final value
    /// does not satisfy `predicate`.
    pub async fn wait_for(&self, mut predicate: impl FnMut(&T) -> bool) -> Result<WatchRef<'_, T>, Error> {
        loop {
            {
                let state = self.shared.state();
                self.observed.store(state.version, Ordering::Release);
                if predicate(&state.value) {
                    telemetry::record(EventKind::ChannelReceive, std::ptr::from_ref(&*self.shared).cast::<()>());
                    return Ok(WatchRef { guard: state });
                }
                if state.senders == 0 {
                    return Err(Error::without_value(ERROR_CLOSED));
                }
            }
            self.changed().await?;
        }
    }

    /// Blocks the current thread until a newer version is available.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when all senders are gone and no newer
    /// version remains unobserved.
    pub fn changed_sync(&self) -> Result<(), Error> {
        block_on(self.changed())
    }

    /// Returns whether all senders have been dropped.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.shared.state().senders == 0
    }

    fn change_ready(&self) -> bool {
        self.change_status() != WatchChange::Pending
    }

    fn change_status(&self) -> WatchChange {
        let state = self.shared.state();
        match (state.version == self.observed.load(Ordering::Acquire), state.senders) {
            (false, _) => WatchChange::Version(state.version),
            (true, 0) => WatchChange::Closed,
            (true, _) => WatchChange::Pending,
        }
    }

    fn observe_change(&self) -> WatchChange {
        let state = self.shared.state();
        let change = match (state.version == self.observed.load(Ordering::Acquire), state.senders) {
            (false, _) => WatchChange::Version(state.version),
            (true, 0) => WatchChange::Closed,
            (true, _) => WatchChange::Pending,
        };
        if let WatchChange::Version(version) = change {
            self.observed.store(version, Ordering::Release);
        }
        change
    }
}

impl<T> Clone for WatchReceiver<T> {
    fn clone(&self) -> Self {
        self.shared.state().receivers += 1;
        Self {
            shared: Arc::clone(&self.shared),
            observed: AtomicU64::new(self.observed.load(Ordering::Acquire)),
        }
    }
}

impl<T> Drop for WatchReceiver<T> {
    fn drop(&mut self) {
        let remaining_receivers = {
            let mut state = self.shared.state();
            state.receivers -= 1;
            state.receivers
        };
        #[expect(clippy::single_match, reason = "only the transition to zero emits channel-close telemetry")]
        match remaining_receivers {
            0 => telemetry::record(EventKind::ChannelClose, std::ptr::from_ref(&*self.shared).cast::<()>()),
            _ => {}
        }
    }
}

impl<T> fmt::Debug for WatchReceiver<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WatchReceiver")
            .field("closed", &self.is_closed())
            .field("changed", &self.has_changed().unwrap_or(false))
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
struct WatchChanged<'a, T> {
    receiver: &'a WatchReceiver<T>,
    waiter: Option<Arc<Waiter>>,
}

impl<T> Future for WatchChanged<'_, T> {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.receiver.change_ready() {
            return Poll::Ready(());
        }
        let waiter = Arc::clone(self.waiter.get_or_insert_with(|| Arc::new(Waiter::new())));
        waiter.register(cx.waker());
        if self
            .receiver
            .shared
            .receiver_waiters
            .enqueue_if_needed(&waiter, || self.receiver.change_ready())
        {
            self.waiter.take();
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }
}

impl<T> Drop for WatchChanged<'_, T> {
    fn drop(&mut self) {
        if let Some(waiter) = &self.waiter {
            self.receiver.shared.receiver_waiters.cancel(waiter);
        }
    }
}

/// Guard that borrows the current value of a watch channel.
///
/// Holding this guard blocks updates through [`WatchSender`].
pub struct WatchRef<'a, T> {
    guard: StdMutexGuard<'a, WatchState<T>>,
}

impl<T> Deref for WatchRef<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.guard.value
    }
}

impl<T: fmt::Debug> fmt::Debug for WatchRef<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.guard.value, f)
    }
}

impl<T: fmt::Display> fmt::Display for WatchRef<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.guard.value, f)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::task::{Context, Poll, Wake, Waker};

    use super::{
        ERROR_CLOSED, ERROR_EMPTY, ERROR_FULL, ERROR_TIMEOUT, Error, OneshotShared, OneshotState, QueueState, QueueWait, WaitQueue,
        WatchChange, WatchChanged, bounded, oneshot, watch,
    };

    #[derive(Default)]
    struct WakeCounter(AtomicUsize);

    impl Wake for WakeCounter {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn unknown_error_codes_use_the_defensive_message() {
        let error = Error::<()> {
            code: u8::MAX,
            value: None,
        };

        assert_eq!(error.to_string(), "channel operation failed");
    }

    #[test]
    fn dropping_unpolled_queue_wait_has_no_registration_to_cancel() {
        let (sender, _receiver) = bounded::<()>(1);

        drop(QueueWait::send(&sender.shared));
    }

    #[test]
    fn capacity_change_during_waiter_registration_completes_wait() {
        let (sender, _receiver) = bounded::<()>(1);
        let mut wait = QueueWait::send(&sender.shared);
        let context = Context::from_waker(Waker::noop());

        assert_eq!(wait.poll_registered(&context), Poll::Ready(()));
    }

    #[test]
    fn unavailable_capacity_during_waiter_registration_stays_pending() {
        let (sender, _receiver) = bounded::<()>(1);
        sender.try_send(()).unwrap();
        let mut wait = QueueWait::send(&sender.shared);
        let context = Context::from_waker(Waker::noop());

        assert_eq!(wait.poll_registered(&context), Poll::Pending);
    }

    #[test]
    fn error_predicates_match_only_their_own_error_code() {
        let errors = [ERROR_FULL, ERROR_EMPTY, ERROR_CLOSED, ERROR_TIMEOUT].map(Error::without_value);

        assert_eq!(
            errors.map(|error| { (error.is_full(), error.is_empty(), error.is_closed(), error.is_timeout(),) }),
            [
                (true, false, false, false),
                (false, true, false, false),
                (false, false, true, false),
                (false, false, false, true),
            ]
        );
    }

    #[test]
    fn queue_closure_accounts_for_explicit_close_and_sender_lifetime() {
        let explicitly_closed = QueueState::<()> {
            values: VecDeque::new(),
            capacity: None,
            senders: 1,
            receivers: 1,
            closed: true,
            high_watermark: 0,
        };
        let no_senders = QueueState::<()> {
            values: VecDeque::new(),
            capacity: None,
            closed: false,
            senders: 0,
            receivers: 1,
            high_watermark: 0,
        };

        assert!(explicitly_closed.receive_is_closed());
        assert!(no_senders.receive_is_closed());
    }

    #[test]
    fn queue_try_send_stores_values_and_rejects_full_capacity() {
        let (sender, _receiver) = bounded(1);

        assert_eq!(sender.shared.try_send(7), Ok(()));
        assert_eq!(sender.shared.state().values.front(), Some(&7));
        assert_eq!(sender.shared.try_send(9), Err((ERROR_FULL, 9)));
    }

    #[test]
    fn queue_wait_completion_tracks_capacity_values_and_closure() {
        let (sender, receiver) = bounded(1);
        let shared = Arc::clone(&sender.shared);
        let send_wait = QueueWait::send(&shared);
        let receive_wait = QueueWait::receive(&shared);
        assert!(send_wait.complete());
        assert!(!receive_wait.complete());

        sender.try_send(1).unwrap();
        assert!(!send_wait.complete());
        assert!(receive_wait.complete());
        assert_eq!(receiver.try_recv(), Ok(1));
        drop(sender);
        assert!(receive_wait.complete());
    }

    #[test]
    fn queue_endpoint_closed_state_tracks_the_opposite_endpoint() {
        let (sender, receiver) = bounded::<()>(1);
        assert!(!sender.is_closed());
        assert!(!receiver.is_closed());

        let (sender_without_receiver, receiver) = bounded::<()>(1);
        drop(receiver);
        assert!(sender_without_receiver.is_closed());

        let (sender, receiver_without_sender) = bounded::<()>(1);
        drop(sender);
        assert!(receiver_without_sender.is_closed());
    }

    #[test]
    fn cancelled_selected_queue_waiter_wakes_the_next_waiter() {
        let (sender, receiver) = bounded(1);
        sender.try_send(1).unwrap();
        let first_counter = Arc::new(WakeCounter::default());
        let first_waker = Waker::from(Arc::clone(&first_counter));
        let mut first_context = Context::from_waker(&first_waker);
        let second_counter = Arc::new(WakeCounter::default());
        let second_waker = Waker::from(Arc::clone(&second_counter));
        let mut second_context = Context::from_waker(&second_waker);
        let mut first = Box::pin(QueueWait::send(&sender.shared));
        let mut second = Box::pin(QueueWait::send(&sender.shared));
        assert!(first.as_mut().poll(&mut first_context).is_pending());
        assert!(second.as_mut().poll(&mut second_context).is_pending());

        assert_eq!(receiver.try_recv(), Ok(1));
        assert_eq!(
            (
                first_counter.0.load(Ordering::Relaxed),
                second_counter.0.load(Ordering::Relaxed),
                first.as_ref().get_ref().complete(),
            ),
            (1, 0, true)
        );
        drop(first);

        assert_eq!(second_counter.0.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn cancelling_a_still_queued_waiter_does_not_wake_the_next() {
        let (sender, _receiver) = bounded(1);
        sender.try_send(1).unwrap();
        let first_counter = Arc::new(WakeCounter::default());
        let first_waker = Waker::from(Arc::clone(&first_counter));
        let mut first_context = Context::from_waker(&first_waker);
        let second_counter = Arc::new(WakeCounter::default());
        let second_waker = Waker::from(Arc::clone(&second_counter));
        let mut second_context = Context::from_waker(&second_waker);
        let mut first = Box::pin(QueueWait::send(&sender.shared));
        let mut second = Box::pin(QueueWait::send(&sender.shared));
        assert!(first.as_mut().poll(&mut first_context).is_pending());
        assert!(second.as_mut().poll(&mut second_context).is_pending());
        assert_eq!(sender.shared.state().values.pop_front(), Some(1));

        drop(first);

        assert_eq!(
            (first_counter.0.load(Ordering::Relaxed), second_counter.0.load(Ordering::Relaxed),),
            (0, 0)
        );
    }

    #[test]
    fn oneshot_readiness_tracks_both_value_and_sender_lifetime() {
        let shared = OneshotShared {
            state: std::sync::Mutex::new(OneshotState {
                value: Some(7),
                sender_alive: true,
                receiver_alive: true,
            }),
            receiver_waiters: WaitQueue::new(),
        };
        assert!(shared.receiver_ready());
        shared.state().value = None;
        assert!(!shared.receiver_ready());
        shared.state().sender_alive = false;
        assert!(shared.receiver_ready());
    }

    #[test]
    fn oneshot_take_result_distinguishes_value_pending_and_closed() {
        let (sender, mut receiver) = oneshot();
        assert_eq!(receiver.take_result(), None);
        sender.send(7).unwrap();
        assert_eq!(receiver.take_result(), Some(Ok(7)));

        let (sender, mut receiver) = oneshot::<usize>();
        drop(sender);
        assert!(receiver.take_result().unwrap().unwrap_err().is_closed());
    }

    #[test]
    fn oneshot_closed_state_requires_a_dead_sender_without_a_value() {
        let (sender, receiver) = oneshot::<usize>();
        assert!(!receiver.is_closed());
        sender.send(7).unwrap();
        assert!(!receiver.is_closed());

        let (sender, receiver) = oneshot::<usize>();
        drop(sender);
        assert!(receiver.is_closed());
    }

    #[test]
    fn watch_readiness_tracks_versions_and_sender_lifetime() {
        let (sender, receiver) = watch(1);
        assert!(!receiver.change_ready());
        sender.send(2).unwrap();
        assert!(receiver.change_ready());
        receiver.observed.store(1, Ordering::Release);
        assert!(!receiver.change_ready());
        drop(sender);
        assert!(receiver.change_ready());
    }

    #[test]
    fn watch_change_status_distinguishes_pending_updated_and_closed() {
        let (sender, receiver) = watch(1);
        assert_eq!(receiver.change_status(), WatchChange::Pending);
        sender.send(2).unwrap();
        assert_eq!(receiver.change_status(), WatchChange::Version(1));
        receiver.observed.store(1, Ordering::Release);
        drop(sender);
        assert_eq!(receiver.change_status(), WatchChange::Closed);
    }

    #[test]
    fn observing_a_watch_change_updates_the_version_while_state_is_locked() {
        let (sender, receiver) = watch(1);
        sender.send(2).unwrap();

        assert_eq!(receiver.observe_change(), WatchChange::Version(1));
        assert_eq!(receiver.observed.load(Ordering::Acquire), 1);
        assert_eq!(receiver.observe_change(), WatchChange::Pending);
    }

    #[test]
    fn dropping_watch_endpoints_updates_counts_without_premature_close() {
        let (sender, receiver) = watch(1);
        let sender_clone = sender.clone();
        let receiver_clone = receiver.clone();

        drop(sender);
        assert!(!receiver.is_closed());
        drop(sender_clone);
        assert!(receiver.is_closed());
        drop(receiver_clone);

        let (sender, receiver) = watch(1);
        let receiver_clone = receiver.clone();
        drop(receiver);
        assert!(!sender.is_closed());
        drop(receiver_clone);
        assert!(sender.is_closed());
    }

    #[test]
    fn dropping_watch_changed_releases_its_registered_waker() {
        let (_sender, receiver) = watch(1);
        let counter = Arc::new(WakeCounter::default());
        let weak = Arc::downgrade(&counter);
        {
            let waker = Waker::from(Arc::clone(&counter));
            let mut context = Context::from_waker(&waker);
            let mut changed = Box::pin(WatchChanged {
                receiver: &receiver,
                waiter: None,
            });
            assert!(changed.as_mut().poll(&mut context).is_pending());
        }
        drop(counter);

        assert!(weak.upgrade().is_none());
    }
}
