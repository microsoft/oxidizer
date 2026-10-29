// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll, Waker};
use std::{fmt, io};

use mio::event::Source;
use mio::{Interest, Token};
use performables::arc::Arc;
use performables::sync::mutex::Mutex;

use crate::error::RegistrationClosed;
use crate::telemetry::SourceOperation;
use crate::worker::WorkerState;

const READABLE: u8 = 1 << 0;
const WRITABLE: u8 = 1 << 1;
const ERROR: u8 = 1 << 2;
const READ_CLOSED: u8 = 1 << 3;
const WRITE_CLOSED: u8 = 1 << 4;
const PRIORITY: u8 = 1 << 5;
const AIO: u8 = 1 << 6;
const LIO: u8 = 1 << 7;

const READ_SIGNALS: u8 = READABLE | ERROR | READ_CLOSED | WRITE_CLOSED | PRIORITY | AIO | LIO;
const WRITE_SIGNALS: u8 = WRITABLE | ERROR | READ_CLOSED | WRITE_CLOSED | AIO | LIO;

struct RegistryOperation<'a>(&'a WorkerState);

impl Drop for RegistryOperation<'_> {
    fn drop(&mut self) {
        self.0.finish_registry_operation();
    }
}

pub(crate) struct RegistrationEntry {
    inner: Mutex<RegistrationInner>,
    closed: AtomicBool,
}

impl Default for RegistrationEntry {
    fn default() -> Self {
        Self {
            inner: Mutex::new(RegistrationInner::default()),
            closed: AtomicBool::new(false),
        }
    }
}

impl RegistrationEntry {
    pub(crate) fn notify(&self, event: &mio::event::Event) {
        self.notify_signals(event_signals(event));
    }

    fn notify_signals(&self, signals: u8) {
        let (read_waiter, write_waiter) = {
            let mut inner = self.inner.lock();
            if inner.closed {
                return;
            }
            let wake_read = signals & READ_SIGNALS != 0;
            let wake_write = signals & WRITE_SIGNALS != 0;
            if wake_read {
                inner.read_ready = true;
            }
            if wake_write {
                inner.write_ready = true;
            }
            (
                wake_read.then(|| inner.read_waiter.take()).flatten(),
                wake_write.then(|| inner.write_waiter.take()).flatten(),
            )
        };
        if let Some(waiter) = read_waiter {
            waiter.wake();
        }
        if let Some(waiter) = write_waiter {
            waiter.wake();
        }
    }

    pub(crate) fn close(&self) {
        self.closed.store(true, Ordering::Release);
        let (read_waiter, write_waiter) = {
            let mut inner = self.inner.lock();
            inner.closed = true;
            (inner.read_waiter.take(), inner.write_waiter.take())
        };
        if let Some(waiter) = read_waiter {
            waiter.wake();
        }
        if let Some(waiter) = write_waiter {
            waiter.wake();
        }
    }

    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    fn poll_read(&self, waker: &Waker) -> Poll<Result<(), RegistrationClosed>> {
        let mut inner = self.inner.lock();
        if std::mem::take(&mut inner.read_ready) {
            return Poll::Ready(Ok(()));
        }
        if inner.closed {
            return Poll::Ready(Err(RegistrationClosed));
        }
        if inner.read_waiter.as_ref().is_none_or(|waiter| !waiter.will_wake(waker)) {
            inner.read_waiter = Some(waker.clone());
        }
        Poll::Pending
    }

    fn poll_write(&self, waker: &Waker) -> Poll<Result<(), RegistrationClosed>> {
        let mut inner = self.inner.lock();
        if std::mem::take(&mut inner.write_ready) {
            return Poll::Ready(Ok(()));
        }
        if inner.closed {
            return Poll::Ready(Err(RegistrationClosed));
        }
        if inner.write_waiter.as_ref().is_none_or(|waiter| !waiter.will_wake(waker)) {
            inner.write_waiter = Some(waker.clone());
        }
        Poll::Pending
    }
}

#[derive(Default)]
struct RegistrationInner {
    read_ready: bool,
    write_ready: bool,
    read_waiter: Option<Waker>,
    write_waiter: Option<Waker>,
    closed: bool,
}

/// One direct-ownership registration for a Mio source.
///
/// The owner accesses the source through ordinary Rust references. The source
/// is not behind a mutex. Driver shutdown closes readiness and the native
/// poller, but this value retains the source until it drops.
///
/// Registration and deregistration use a clone from
/// [`Registry::try_clone`](mio::Registry::try_clone). They run on the thread
/// that calls [`MioContext::register`](crate::MioContext::register) or drops
/// this value.
pub struct Registered<S>
where
    S: Source + Send + 'static,
{
    source: S,
    worker: Arc<WorkerState>,
    token: Token,
    entry: Arc<RegistrationEntry>,
}

impl<S> fmt::Debug for Registered<S>
where
    S: Source + Send + fmt::Debug + 'static,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Registered").field("token", &self.token).finish_non_exhaustive()
    }
}

impl<S> Registered<S>
where
    S: Source + Send + 'static,
{
    fn new(source: S, worker: Arc<WorkerState>, token: Token, entry: Arc<RegistrationEntry>) -> Self {
        Self {
            source,
            worker,
            token,
            entry,
        }
    }

    /// Returns the Mio token assigned to this source.
    #[must_use]
    pub const fn token(&self) -> Token {
        self.token
    }

    /// Returns a shared reference to the source while the driver is open.
    ///
    /// # Errors
    ///
    /// Returns [`RegistrationClosed`] if the driver is closed.
    pub fn source(&self) -> Result<&S, RegistrationClosed> {
        if self.entry.is_closed() {
            Err(RegistrationClosed)
        } else {
            Ok(&self.source)
        }
    }

    /// Returns an exclusive reference to the source while the driver is open.
    ///
    /// Read or write until the operation returns
    /// [`WouldBlock`](io::ErrorKind::WouldBlock). Then wait for new readiness.
    ///
    /// A source operation that starts before driver shutdown can finish after
    /// shutdown starts. The driver never accesses or drops the source.
    ///
    /// # Errors
    ///
    /// Returns [`RegistrationClosed`] if the driver is closed.
    pub fn source_mut(&mut self) -> Result<&mut S, RegistrationClosed> {
        if self.entry.is_closed() {
            Err(RegistrationClosed)
        } else {
            Ok(&mut self.source)
        }
    }

    /// Polls for read readiness or driver shutdown.
    ///
    /// Call this method only after a read operation returns
    /// [`WouldBlock`](io::ErrorKind::WouldBlock). A readiness signal is a hint
    /// and can be spurious. A ready result contains `()` and consumes only the
    /// accumulated read readiness.
    ///
    /// # Errors
    ///
    /// Returns [`RegistrationClosed`] if the driver closes before it reports
    /// more read readiness.
    #[expect(
        clippy::needless_pass_by_ref_mut,
        reason = "the poll API follows Future::poll and accepts the caller's task context"
    )]
    pub fn poll_read_ready(&self, context: &mut Context<'_>) -> Poll<Result<(), RegistrationClosed>> {
        self.entry.poll_read(context.waker())
    }

    /// Polls for write readiness or driver shutdown.
    ///
    /// Call this method only after a write operation returns
    /// [`WouldBlock`](io::ErrorKind::WouldBlock). A readiness signal is a hint
    /// and can be spurious. A ready result contains `()` and consumes only the
    /// accumulated write readiness.
    ///
    /// # Errors
    ///
    /// Returns [`RegistrationClosed`] if the driver closes before it reports
    /// more write readiness.
    #[expect(
        clippy::needless_pass_by_ref_mut,
        reason = "the poll API follows Future::poll and accepts the caller's task context"
    )]
    pub fn poll_write_ready(&self, context: &mut Context<'_>) -> Poll<Result<(), RegistrationClosed>> {
        self.entry.poll_write(context.waker())
    }
}

fn event_signals(event: &mio::event::Event) -> u8 {
    (u8::from(event.is_readable()) * READABLE)
        | (u8::from(event.is_writable()) * WRITABLE)
        | (u8::from(event.is_error()) * ERROR)
        | (u8::from(event.is_read_closed()) * READ_CLOSED)
        | (u8::from(event.is_write_closed()) * WRITE_CLOSED)
        | (u8::from(event.is_priority()) * PRIORITY)
        | (u8::from(event.is_aio()) * AIO)
        | (u8::from(event.is_lio()) * LIO)
}

impl<S> Drop for Registered<S>
where
    S: Source + Send + 'static,
{
    fn drop(&mut self) {
        if !self.worker.begin_deregistration(self.token, &self.entry) {
            self.entry.close();
            return;
        }
        let _operation = RegistryOperation(&self.worker);
        if let Err(error) = self.worker.registry().deregister(&mut self.source) {
            self.worker.report_source_failure(SourceOperation::Deregistration, &error);
            self.worker.record_failure(error);
        }
        self.entry.close();
    }
}

pub(crate) fn register<S>(mut source: S, interest: Interest, worker: Arc<WorkerState>) -> io::Result<Registered<S>>
where
    S: Source + Send + 'static,
{
    let token = worker
        .allocate_token()
        .inspect_err(|error| worker.report_source_failure(SourceOperation::Registration, error))?;
    let entry = Arc::new(RegistrationEntry::default());
    worker
        .begin_registration(token, Arc::clone(&entry))
        .inspect_err(|error| worker.report_source_failure(SourceOperation::Registration, error))?;
    let operation = RegistryOperation(&worker);
    if let Err(error) = worker.registry().register(&mut source, token, interest) {
        worker.finish_registration(token, &entry, false);
        entry.close();
        worker.report_source_failure(SourceOperation::Registration, &error);
        return Err(error);
    }
    if !worker.finish_registration(token, &entry, true) {
        if let Err(error) = worker.registry().deregister(&mut source) {
            worker.report_source_failure(SourceOperation::Deregistration, &error);
            worker.record_failure(error);
        }
        let error = io::Error::new(
            io::ErrorKind::BrokenPipe,
            "the Arty Mio driver closed while it registered the source",
        );
        worker.report_source_failure(SourceOperation::Registration, &error);
        return Err(error);
    }
    drop(operation);
    Ok(Registered::new(source, worker, token, entry))
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::sync::mpsc;
    use std::task::Waker;

    use mio::event::Source;
    use mio::{Interest, Poll, Registry, Token};
    use performables::arc::Arc;

    use super::{
        AIO, ERROR, LIO, PRIORITY, READ_CLOSED, READ_SIGNALS, READABLE, RegistrationEntry, WRITABLE, WRITE_CLOSED, WRITE_SIGNALS, register,
    };
    use crate::worker::WorkerState;

    #[test]
    fn mio_signals_map_to_the_expected_directions() {
        for (signal, read, write) in [
            (READABLE, true, false),
            (WRITABLE, false, true),
            (ERROR, true, true),
            (READ_CLOSED, true, true),
            (WRITE_CLOSED, true, true),
            (PRIORITY, true, false),
            (AIO, true, true),
            (LIO, true, true),
        ] {
            assert_eq!(signal & READ_SIGNALS != 0, read);
            assert_eq!(signal & WRITE_SIGNALS != 0, write);
        }
    }

    #[test]
    fn a_shared_signal_sets_both_bits_and_each_poll_takes_only_its_bit() {
        let state = RegistrationEntry::default();
        state.notify_signals(ERROR);

        assert_eq!(state.poll_read(Waker::noop()), std::task::Poll::Ready(Ok(())));
        assert_eq!(state.poll_read(Waker::noop()), std::task::Poll::Pending);
        assert_eq!(state.poll_write(Waker::noop()), std::task::Poll::Ready(Ok(())));
        assert_eq!(state.poll_write(Waker::noop()), std::task::Poll::Pending);
    }

    #[test]
    fn repeated_signals_coalesce_into_one_pending_bit() {
        let state = RegistrationEntry::default();
        state.notify_signals(READABLE);
        state.notify_signals(READABLE);

        assert_eq!(state.poll_read(Waker::noop()), std::task::Poll::Ready(Ok(())));
        assert_eq!(state.poll_read(Waker::noop()), std::task::Poll::Pending);
    }

    #[test]
    fn shutdown_wins_against_an_in_flight_registration() {
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (deregister_tx, deregister_rx) = mpsc::channel();
        let poll = Poll::new().unwrap();
        let worker = Arc::new(WorkerState::new(
            std::thread::current().id(),
            poll.registry().try_clone().unwrap(),
            Waker::noop().clone(),
            observed::Sink::noop(),
        ));
        let registration_worker = Arc::clone(&worker);
        let registration_thread = std::thread::spawn(move || {
            register(
                BlockingSource {
                    entered_tx,
                    release_rx,
                    deregister_tx,
                },
                Interest::READABLE,
                registration_worker,
            )
        });
        let registration_thread_id = registration_thread.thread().id();

        entered_rx.recv().unwrap();
        let shutdown_worker = Arc::clone(&worker);
        let shutdown_thread = std::thread::spawn(move || shutdown_worker.close_registrations());
        while !worker.registrations_are_closed() {
            std::thread::yield_now();
        }
        release_tx.send(()).unwrap();

        assert_eq!(
            registration_thread.join().unwrap().err().unwrap().kind(),
            std::io::ErrorKind::BrokenPipe
        );
        shutdown_thread.join().unwrap();
        assert_eq!(deregister_rx.recv().unwrap(), registration_thread_id);
    }

    struct BlockingSource {
        entered_tx: mpsc::Sender<()>,
        release_rx: mpsc::Receiver<()>,
        deregister_tx: mpsc::Sender<std::thread::ThreadId>,
    }

    impl Source for BlockingSource {
        fn register(&mut self, _registry: &Registry, _token: Token, _interests: Interest) -> std::io::Result<()> {
            self.entered_tx.send(()).unwrap();
            self.release_rx.recv().unwrap();
            Ok(())
        }

        fn reregister(&mut self, _registry: &Registry, _token: Token, _interests: Interest) -> std::io::Result<()> {
            Ok(())
        }

        fn deregister(&mut self, _registry: &Registry) -> std::io::Result<()> {
            self.deregister_tx.send(std::thread::current().id()).unwrap();
            Ok(())
        }
    }
}
