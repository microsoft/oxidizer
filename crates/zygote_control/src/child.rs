// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#[cfg(all(test, unix))]
use std::cell::Cell;
use std::fmt::{self, Debug, Formatter};
use std::io::{self, Read, Write};
#[cfg(target_os = "linux")]
use std::os::fd::{AsFd, BorrowedFd};
#[cfg(unix)]
use std::os::fd::{AsRawFd, RawFd};
#[cfg(windows)]
use std::os::windows::io::{AsRawHandle, RawHandle};
use std::process::{ExitStatus, Output};
#[cfg(windows)]
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(windows)]
use windows_sys::Win32::Foundation::{ERROR_BROKEN_PIPE, ERROR_HANDLE_EOF, HANDLE};
#[cfg(windows)]
use windows_sys::Win32::System::Pipes::PeekNamedPipe;

#[cfg(target_os = "linux")]
use crate::linux::LinuxChild;
#[cfg(not(target_os = "linux"))]
use crate::zygote::HealthState;

#[cfg(all(test, unix))]
thread_local! {
    static FAIL_OUTPUT_COLLECTOR_START: Cell<bool> = const { Cell::new(false) };
}

#[cfg(unix)]
pub(super) trait ReadPipe: Read + Send {
    fn raw_fd(&self) -> RawFd;
}

#[cfg(unix)]
impl<T: Read + Send + AsRawFd> ReadPipe for T {
    fn raw_fd(&self) -> RawFd {
        self.as_raw_fd()
    }
}

#[cfg(windows)]
pub(super) trait ReadPipe: Read + Send {
    fn raw_handle(&self) -> RawHandle;
}

#[cfg(windows)]
impl<T: Read + Send + AsRawHandle> ReadPipe for T {
    fn raw_handle(&self) -> RawHandle {
        self.as_raw_handle()
    }
}

#[cfg(all(not(unix), not(windows)))]
pub(super) trait ReadPipe: Read + Send {}

#[cfg(all(not(unix), not(windows)))]
impl<T: Read + Send> ReadPipe for T {}

pub(super) enum ChildInner {
    #[cfg(not(any(target_os = "linux", windows)))]
    Native(std::process::Child),
    #[cfg(windows)]
    Windows(crate::windows::WindowsChild),
    #[cfg(target_os = "linux")]
    Linux(LinuxChild),
}

/// Finite byte limits used while capturing child output.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OutputLimits {
    per_stream: usize,
    aggregate: usize,
}

impl OutputLimits {
    /// Creates output limits.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::InvalidInput`] when either limit is zero or
    /// the aggregate limit is smaller than the per-stream limit.
    pub fn new(per_stream: usize, aggregate: usize) -> io::Result<Self> {
        if per_stream == 0 || aggregate < per_stream {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "output limits require nonzero per-stream and sufficient aggregate capacity",
            ));
        }
        Ok(Self { per_stream, aggregate })
    }

    /// Returns the byte limit for either individual stream.
    #[must_use]
    pub const fn per_stream(self) -> usize {
        self.per_stream
    }

    /// Returns the combined stdout and stderr byte limit.
    #[must_use]
    pub const fn aggregate(self) -> usize {
        self.aggregate
    }
}

impl Default for OutputLimits {
    fn default() -> Self {
        Self {
            per_stream: 8 * 1024 * 1024,
            aggregate: 16 * 1024 * 1024,
        }
    }
}

/// A writable pipe connected to a child's standard input.
pub struct ChildStdin {
    pub(super) inner: Box<dyn Write + Send>,
}

impl Debug for ChildStdin {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChildStdin").finish_non_exhaustive()
    }
}

impl Write for ChildStdin {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// A readable pipe connected to a child's standard output.
pub struct ChildStdout {
    pub(super) inner: Box<dyn ReadPipe>,
}

impl Debug for ChildStdout {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChildStdout").finish_non_exhaustive()
    }
}

impl Read for ChildStdout {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }
}

/// A readable pipe connected to a child's standard error.
pub struct ChildStderr {
    pub(super) inner: Box<dyn ReadPipe>,
}

impl Debug for ChildStderr {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChildStderr").finish_non_exhaustive()
    }
}

impl Read for ChildStderr {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }
}

/// A running or completed launched process.
pub struct Child {
    pub(super) inner: ChildInner,
    /// A pipe connected to the child's standard input, when requested.
    pub stdin: Option<ChildStdin>,
    /// A pipe connected to the child's standard output, when requested.
    pub stdout: Option<ChildStdout>,
    /// A pipe connected to the child's standard error, when requested.
    pub stderr: Option<ChildStderr>,
    pub(super) output_limits: OutputLimits,
    #[cfg(not(target_os = "linux"))]
    pub(super) active_health: Option<Arc<HealthState>>,
}

impl Debug for Child {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Child").field("id", &self.id()).finish_non_exhaustive()
    }
}

impl Child {
    /// Returns the operating-system process identifier.
    #[must_use]
    pub fn id(&self) -> u32 {
        match &self.inner {
            #[cfg(not(any(target_os = "linux", windows)))]
            ChildInner::Native(child) => child.id(),
            #[cfg(windows)]
            ChildInner::Windows(child) => child.id(),
            #[cfg(target_os = "linux")]
            ChildInner::Linux(child) => child.id(),
        }
    }

    /// Terminates the child.
    ///
    /// # Errors
    ///
    /// Returns an error if the operating system cannot signal the child.
    pub fn kill(&mut self) -> io::Result<()> {
        match &mut self.inner {
            #[cfg(not(any(target_os = "linux", windows)))]
            ChildInner::Native(child) => child.kill(),
            #[cfg(windows)]
            ChildInner::Windows(child) => child.kill(),
            #[cfg(target_os = "linux")]
            ChildInner::Linux(child) => child.kill(),
        }
    }

    /// Waits for the child to terminate.
    ///
    /// # Errors
    ///
    /// Returns an error if waiting fails or the zygote detached before
    /// reporting terminal status.
    pub fn wait(&mut self) -> io::Result<ExitStatus> {
        drop(self.stdin.take());
        let result = match &mut self.inner {
            #[cfg(not(any(target_os = "linux", windows)))]
            ChildInner::Native(child) => child.wait(),
            #[cfg(windows)]
            ChildInner::Windows(child) => child.wait(),
            #[cfg(target_os = "linux")]
            ChildInner::Linux(child) => child.wait(),
        };
        #[cfg(not(target_os = "linux"))]
        if result.is_ok() {
            self.finish_native_tracking();
        }
        result
    }

    /// Returns the child's status if it has terminated.
    ///
    /// # Errors
    ///
    /// Returns an error if status observation fails or the zygote detached
    /// before reporting terminal status.
    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        let result = match &mut self.inner {
            #[cfg(not(any(target_os = "linux", windows)))]
            ChildInner::Native(child) => child.try_wait(),
            #[cfg(windows)]
            ChildInner::Windows(child) => child.try_wait(),
            #[cfg(target_os = "linux")]
            ChildInner::Linux(child) => child.try_wait(),
        };
        #[cfg(not(target_os = "linux"))]
        if matches!(result, Ok(Some(_))) {
            self.finish_native_tracking();
        }
        result
    }

    fn try_wait_for_output(&mut self) -> io::Result<Option<ExitStatus>> {
        let result = match &mut self.inner {
            #[cfg(not(any(target_os = "linux", windows)))]
            ChildInner::Native(child) => child.try_wait(),
            #[cfg(windows)]
            ChildInner::Windows(child) => child.try_wait(),
            #[cfg(target_os = "linux")]
            ChildInner::Linux(child) => child.try_wait_for_output(),
        };
        #[cfg(not(target_os = "linux"))]
        if matches!(result, Ok(Some(_))) {
            self.finish_native_tracking();
        }
        result
    }

    #[cfg(not(target_os = "linux"))]
    fn finish_native_tracking(&mut self) {
        if let Some(health) = self.active_health.take() {
            let _ = health
                .active_children
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |active| Some(active.saturating_sub(1)));
        }
    }

    /// Waits for termination and collects configured output pipes.
    ///
    /// Capture is bounded by the command's [`OutputLimits`]. If either stream
    /// or their aggregate exceeds its limit, collection is cancelled, read
    /// handles are dropped, and bounded child cleanup is attempted.
    ///
    /// # Errors
    ///
    /// Returns an error if waiting or reading either captured stream fails.
    pub fn wait_with_output(mut self) -> io::Result<Output> {
        drop(self.stdin.take());
        if self.stdout.is_none() && self.stderr.is_none() {
            let status = self.wait()?;
            return Ok(Output {
                status,
                stdout: Vec::new(),
                stderr: Vec::new(),
            });
        }

        let (collector, limit_exceeded, budget) = match spawn_output_collector(self.stdout.take(), self.stderr.take(), self.output_limits) {
            Ok(collector) => collector,
            Err(start_error) => {
                start_error.budget.cancel();
                let cleanup = terminate_after_output_error(&mut self);
                let collector = join_output_collector(start_error.collector);
                return Err(combine_output_errors(start_error.error, cleanup.err(), collector.err()));
            }
        };

        let status = loop {
            if limit_exceeded.try_recv().is_ok() {
                return Err(handle_output_notification(&mut self, &budget, collector));
            }
            match self.try_wait_for_output() {
                Ok(Some(status)) => break status,
                Ok(None) => match limit_exceeded.recv_timeout(Duration::from_millis(10)) {
                    Ok(()) => {
                        return Err(handle_output_notification(&mut self, &budget, collector));
                    }
                    Err(mpsc::RecvTimeoutError::Timeout | mpsc::RecvTimeoutError::Disconnected) => {}
                },
                Err(error) => {
                    budget.cancel();
                    let cleanup = terminate_after_output_error(&mut self);
                    let collector = join_output_collector(collector);
                    return Err(combine_output_errors(error, cleanup.err(), collector.err()));
                }
            }
        };
        let (stdout, stderr) = join_output_collector(collector)?;
        Ok(Output { status, stdout, stderr })
    }
}

#[cfg(unix)]
type OutputCollector = Option<thread::JoinHandle<io::Result<(Vec<u8>, Vec<u8>)>>>;

#[cfg(not(unix))]
#[derive(Debug)]
struct OutputCollector {
    stdout: Option<thread::JoinHandle<io::Result<Vec<u8>>>>,
    stderr: Option<thread::JoinHandle<io::Result<Vec<u8>>>>,
}

#[derive(Debug)]
struct CollectorStartError {
    error: io::Error,
    collector: OutputCollector,
    budget: Arc<CaptureBudget>,
}

#[derive(Debug)]
struct CaptureBudget {
    limits: OutputLimits,
    aggregate: AtomicUsize,
    exceeded: AtomicBool,
    cancelled: AtomicBool,
    notify: mpsc::Sender<()>,
}

impl CaptureBudget {
    fn account(&self, stream_total: usize, count: usize) -> bool {
        let aggregate = self.aggregate.fetch_add(count, Ordering::Relaxed).saturating_add(count);
        let allowed = stream_total.saturating_add(count) <= self.limits.per_stream && aggregate <= self.limits.aggregate;
        if !allowed && !self.exceeded.swap(true, Ordering::Relaxed) {
            self.cancel();
            let _ = self.notify.send(());
        }
        allowed
    }

    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    fn error(&self) -> Option<io::Error> {
        self.exceeded
            .load(Ordering::Relaxed)
            .then(|| io::Error::other("captured child output exceeded configured limits"))
    }
}

#[cfg(not(target_os = "linux"))]
impl Drop for Child {
    fn drop(&mut self) {
        self.finish_native_tracking();
    }
}

#[cfg(unix)]
fn spawn_output_collector(
    stdout: Option<ChildStdout>,
    stderr: Option<ChildStderr>,
    limits: OutputLimits,
) -> Result<(OutputCollector, mpsc::Receiver<()>, Arc<CaptureBudget>), CollectorStartError> {
    let (notify, exceeded) = mpsc::channel();
    let budget = Arc::new(CaptureBudget {
        limits,
        aggregate: AtomicUsize::new(0),
        exceeded: AtomicBool::new(false),
        cancelled: AtomicBool::new(false),
        notify,
    });
    if stdout.is_none() && stderr.is_none() {
        return Ok((None, exceeded, budget));
    }
    #[cfg(test)]
    if FAIL_OUTPUT_COLLECTOR_START.with(|flag| flag.replace(false)) {
        return Err(CollectorStartError {
            error: io::Error::other("injected output collector start failure"),
            collector: None,
            budget,
        });
    }
    let collector_budget = Arc::clone(&budget);
    thread::Builder::new()
        .name("zygote-output".to_owned())
        .spawn(move || {
            let result = collect_unix_output(stdout, stderr, &collector_budget);
            if result.is_err() {
                let _ = collector_budget.notify.send(());
            }
            result
        })
        .map(|collector| (Some(collector), exceeded, Arc::clone(&budget)))
        .map_err(|error| CollectorStartError {
            error,
            collector: None,
            budget,
        })
}

#[cfg(not(unix))]
fn spawn_output_collector(
    stdout: Option<ChildStdout>,
    stderr: Option<ChildStderr>,
    limits: OutputLimits,
) -> Result<(OutputCollector, mpsc::Receiver<()>, Arc<CaptureBudget>), CollectorStartError> {
    let (notify, exceeded) = mpsc::channel();
    let budget = Arc::new(CaptureBudget {
        limits,
        aggregate: AtomicUsize::new(0),
        exceeded: AtomicBool::new(false),
        cancelled: AtomicBool::new(false),
        notify,
    });
    let stdout =
        spawn_reader("zygote-stdout", stdout.map(|stream| stream.inner), Arc::clone(&budget)).map_err(|error| CollectorStartError {
            error,
            collector: OutputCollector {
                stdout: None,
                stderr: None,
            },
            budget: Arc::clone(&budget),
        })?;
    let stderr = match spawn_reader("zygote-stderr", stderr.map(|stream| stream.inner), Arc::clone(&budget)) {
        Ok(stderr) => stderr,
        Err(error) => {
            budget.cancel();
            return Err(CollectorStartError {
                error,
                collector: OutputCollector { stdout, stderr: None },
                budget,
            });
        }
    };
    Ok((OutputCollector { stdout, stderr }, exceeded, budget))
}

#[cfg(windows)]
fn spawn_reader(
    name: &str,
    pipe: Option<Box<dyn ReadPipe>>,
    budget: Arc<CaptureBudget>,
) -> io::Result<Option<thread::JoinHandle<io::Result<Vec<u8>>>>> {
    pipe.map(|mut pipe| {
        thread::Builder::new().name(name.to_owned()).spawn(move || {
            let result = (|| {
                let mut bytes = Vec::new();
                let mut buffer = [0; 16 * 1024];
                while !budget.is_cancelled() {
                    let mut available = 0;
                    // SAFETY: pipe owns the live anonymous-pipe handle for this
                    // call, and available is writable.
                    let result = unsafe {
                        PeekNamedPipe(
                            pipe.raw_handle().cast::<core::ffi::c_void>() as HANDLE,
                            ptr::null_mut(),
                            0,
                            ptr::null_mut(),
                            &raw mut available,
                            ptr::null_mut(),
                        )
                    };
                    if result == 0 {
                        let error = io::Error::last_os_error();
                        if error
                            .raw_os_error()
                            .is_some_and(|code| code == ERROR_BROKEN_PIPE.cast_signed() || code == ERROR_HANDLE_EOF.cast_signed())
                        {
                            break;
                        }
                        return Err(error);
                    }
                    if available == 0 {
                        thread::sleep(Duration::from_millis(1));
                        continue;
                    }
                    let read_len = buffer.len().min(available as usize);
                    let count = pipe.read(&mut buffer[..read_len])?;
                    if count == 0 {
                        break;
                    }
                    if budget.account(bytes.len(), count) {
                        bytes.extend_from_slice(&buffer[..count]);
                    }
                }
                match budget.error() {
                    Some(error) => Err(error),
                    None => Ok(bytes),
                }
            })();
            if result.is_err() {
                let _ = budget.notify.send(());
            }
            result
        })
    })
    .transpose()
}

#[cfg(all(not(unix), not(windows)))]
fn spawn_reader(
    name: &str,
    pipe: Option<Box<dyn ReadPipe>>,
    budget: Arc<CaptureBudget>,
) -> io::Result<Option<thread::JoinHandle<io::Result<Vec<u8>>>>> {
    pipe.map(|mut pipe| {
        thread::Builder::new().name(name.to_owned()).spawn(move || {
            let result = (|| {
                let mut bytes = Vec::new();
                let mut buffer = [0; 16 * 1024];
                while !budget.is_cancelled() {
                    let count = pipe.read(&mut buffer)?;
                    if count == 0 {
                        break;
                    }
                    if budget.account(bytes.len(), count) {
                        bytes.extend_from_slice(&buffer[..count]);
                    }
                }
                match budget.error() {
                    Some(error) => Err(error),
                    None => Ok(bytes),
                }
            })();
            if result.is_err() {
                let _ = budget.notify.send(());
            }
            result
        })
    })
    .transpose()
}

#[cfg(unix)]
#[expect(clippy::too_many_lines, reason = "the two-stream poll state machine is kept together")]
fn collect_unix_output(stdout: Option<ChildStdout>, stderr: Option<ChildStderr>, budget: &CaptureBudget) -> io::Result<(Vec<u8>, Vec<u8>)> {
    struct Stream {
        pipe: Box<dyn ReadPipe>,
        bytes: Vec<u8>,
        finished: bool,
    }

    fn set_nonblocking(stream: &Stream) -> io::Result<()> {
        let flags = unsafe {
            // SAFETY: raw_fd remains owned by stream for this call.
            libc::fcntl(stream.pipe.raw_fd(), libc::F_GETFL)
        };
        if flags < 0 {
            return Err(io::Error::last_os_error());
        }
        let result = unsafe {
            // SAFETY: F_SETFL updates status flags on the owned pipe.
            libc::fcntl(stream.pipe.raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK)
        };
        if result < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    fn drain(stream: &mut Stream, budget: &CaptureBudget) -> io::Result<()> {
        let mut buffer = [0; 16 * 1024];
        loop {
            if budget.is_cancelled() {
                return Ok(());
            }
            match stream.pipe.read(&mut buffer) {
                Ok(0) => {
                    stream.finished = true;
                    return Ok(());
                }
                Ok(count) => {
                    if !budget.account(stream.bytes.len(), count) {
                        return Ok(());
                    }
                    stream.bytes.extend_from_slice(&buffer[..count]);
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error),
            }
        }
    }

    let mut streams = [
        stdout.map(|stream| Stream {
            pipe: stream.inner,
            bytes: Vec::new(),
            finished: false,
        }),
        stderr.map(|stream| Stream {
            pipe: stream.inner,
            bytes: Vec::new(),
            finished: false,
        }),
    ];
    for stream in streams.iter().flatten() {
        set_nonblocking(stream)?;
    }
    let mut first_error = None;
    while !budget.is_cancelled() && streams.iter().flatten().any(|stream| !stream.finished) {
        let mut descriptors = [
            libc::pollfd {
                fd: streams[0].as_ref().map_or(-1, |stream| stream.pipe.raw_fd()),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: streams[1].as_ref().map_or(-1, |stream| stream.pipe.raw_fd()),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        for (descriptor, stream) in descriptors.iter_mut().zip(&streams) {
            if stream.as_ref().is_some_and(|stream| stream.finished) {
                descriptor.fd = -1;
            }
        }
        let result = unsafe {
            // SAFETY: descriptors is valid for both poll entries.
            libc::poll(descriptors.as_mut_ptr(), descriptors.len() as libc::nfds_t, 10)
        };
        if result < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        for index in 0..streams.len() {
            if descriptors[index].revents != 0
                && let Some(stream) = &mut streams[index]
                && let Err(error) = drain(stream, budget)
            {
                if first_error.is_none() {
                    first_error = Some(error);
                    let _ = budget.notify.send(());
                }
                streams[index] = None;
            }
        }
        if budget.is_cancelled() {
            streams = [None, None];
        }
    }

    if let Some(error) = first_error.or_else(|| budget.error()) {
        return Err(error);
    }
    let [stdout, stderr] = streams;
    Ok((
        stdout.map_or_else(Vec::new, |stream| stream.bytes),
        stderr.map_or_else(Vec::new, |stream| stream.bytes),
    ))
}

fn terminate_after_output_error(child: &mut Child) -> io::Result<()> {
    const CLEANUP_TIMEOUT: Duration = Duration::from_secs(5);
    let kill_error = child.kill().err();
    let deadline = Instant::now() + CLEANUP_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_status)) => return kill_error.map_or(Ok(()), Err),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                let timeout = io::Error::new(io::ErrorKind::TimedOut, "child did not terminate within output cleanup deadline");
                return Err(match kill_error {
                    Some(error) => io::Error::other(format!("{error}; {timeout}")),
                    None => timeout,
                });
            }
            Err(wait_error) => {
                return Err(match kill_error {
                    Some(kill_error) => io::Error::other(format!("{kill_error}; status observation failed: {wait_error}")),
                    None => wait_error,
                });
            }
        }
    }
}

fn handle_output_notification(child: &mut Child, budget: &CaptureBudget, collector: OutputCollector) -> io::Error {
    let limit_error = budget.error();
    budget.cancel();
    let cleanup = terminate_after_output_error(child);
    let collector_error = join_output_collector(collector).err();
    match (limit_error, collector_error) {
        (Some(error), collector_error) => combine_output_errors(error, cleanup.err(), collector_error),
        (None, Some(error)) => combine_output_errors(error, cleanup.err(), None),
        (None, None) => combine_output_errors(
            io::Error::other("captured child output notification was inconsistent"),
            cleanup.err(),
            None,
        ),
    }
}

fn combine_output_errors(primary: io::Error, cleanup: Option<io::Error>, collector: Option<io::Error>) -> io::Error {
    let collector = collector.filter(|collector| collector.to_string() != primary.to_string());
    if cleanup.is_none() && collector.is_none() {
        return primary;
    }
    let mut message = primary.to_string();
    if let Some(cleanup) = cleanup {
        message.push_str("; child cleanup failed: ");
        message.push_str(&cleanup.to_string());
    }
    if let Some(collector) = collector {
        message.push_str("; output collector shutdown failed: ");
        message.push_str(&collector.to_string());
    }
    io::Error::new(primary.kind(), message)
}

#[cfg(target_os = "linux")]
impl AsFd for Child {
    fn as_fd(&self) -> BorrowedFd<'_> {
        let ChildInner::Linux(child) = &self.inner;
        child.as_fd()
    }
}

#[cfg(unix)]
fn join_output_collector(handle: OutputCollector) -> io::Result<(Vec<u8>, Vec<u8>)> {
    let Some(handle) = handle else {
        return Ok((Vec::new(), Vec::new()));
    };
    handle.join().map_err(|_panic| io::Error::other("child output reader panicked"))?
}

#[cfg(not(unix))]
fn join_output_collector(collector: OutputCollector) -> io::Result<(Vec<u8>, Vec<u8>)> {
    let stdout = join_reader(collector.stdout);
    let stderr = join_reader(collector.stderr);
    Ok((stdout?, stderr?))
}

#[cfg(not(unix))]
fn join_reader(handle: Option<thread::JoinHandle<io::Result<Vec<u8>>>>) -> io::Result<Vec<u8>> {
    let Some(handle) = handle else {
        return Ok(Vec::new());
    };
    handle.join().map_err(|_panic| io::Error::other("child output reader panicked"))?
}

#[cfg(all(test, not(unix)))]
#[cfg_attr(coverage_nightly, coverage(off))]
mod non_unix_tests {
    use super::*;

    #[test]
    fn capture_budget_enforces_stream_limit_and_cancels_readers() {
        let (notify, exceeded) = mpsc::channel();
        let budget = CaptureBudget {
            limits: OutputLimits::new(4, 8).unwrap(),
            aggregate: AtomicUsize::new(0),
            exceeded: AtomicBool::new(false),
            cancelled: AtomicBool::new(false),
            notify,
        };
        assert!(!budget.account(0, 5));
        assert!(budget.is_cancelled());
        assert!(budget.error().is_some());
        exceeded.try_recv().unwrap();
    }

    #[test]
    fn capture_budget_enforces_the_shared_aggregate_limit() {
        let (notify, exceeded) = mpsc::channel();
        let budget = CaptureBudget {
            limits: OutputLimits::new(8, 9).unwrap(),
            aggregate: AtomicUsize::new(0),
            exceeded: AtomicBool::new(false),
            cancelled: AtomicBool::new(false),
            notify,
        };
        assert!(budget.account(0, 5));
        assert!(!budget.account(0, 5));
        assert!(budget.is_cancelled());
        exceeded.try_recv().unwrap();
    }
}

#[cfg(all(test, unix))]
#[cfg_attr(coverage_nightly, coverage(off))]
#[expect(clippy::unwrap_used, reason = "test failures need no additional context")]
mod tests {
    use std::fs::File;
    use std::os::fd::{FromRawFd, OwnedFd};

    use super::*;

    struct FailingPipe {
        descriptor: OwnedFd,
    }

    impl Read for FailingPipe {
        fn read(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("injected output read failure"))
        }
    }

    impl ReadPipe for FailingPipe {
        fn raw_fd(&self) -> RawFd {
            self.descriptor.as_raw_fd()
        }
    }

    struct TrackingPipe {
        file: File,
        reached_eof: Arc<AtomicBool>,
    }

    impl Read for TrackingPipe {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let count = self.file.read(buf)?;
            if count == 0 {
                self.reached_eof.store(true, Ordering::Relaxed);
            }
            Ok(count)
        }
    }

    impl AsRawFd for TrackingPipe {
        fn as_raw_fd(&self) -> RawFd {
            self.file.as_raw_fd()
        }
    }

    fn pipe_with(contents: &[u8]) -> (OwnedFd, OwnedFd) {
        let mut descriptors = [-1; 2];
        let result = unsafe {
            // SAFETY: descriptors has room for both descriptors.
            libc::pipe2(descriptors.as_mut_ptr(), libc::O_CLOEXEC)
        };
        assert_eq!(result, 0);
        let read = unsafe {
            // SAFETY: pipe2 initialized this owned descriptor.
            OwnedFd::from_raw_fd(descriptors[0])
        };
        let write = unsafe {
            // SAFETY: pipe2 initialized this owned descriptor.
            OwnedFd::from_raw_fd(descriptors[1])
        };
        if !contents.is_empty() {
            let written = unsafe {
                // SAFETY: contents and write are valid for this call.
                libc::write(write.as_raw_fd(), contents.as_ptr().cast(), contents.len())
            };
            assert_eq!(usize::try_from(written).unwrap(), contents.len());
        }
        (read, write)
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn unix_collector_drains_surviving_stream_after_read_failure() {
        let (failing_read, failing_write) = pipe_with(b"x");
        let (tracked_read, tracked_write) = pipe_with(b"survives");
        drop(failing_write);
        drop(tracked_write);
        let reached_eof = Arc::new(AtomicBool::new(false));
        let stdout = ChildStdout {
            inner: Box::new(FailingPipe { descriptor: failing_read }),
        };
        let stderr = ChildStderr {
            inner: Box::new(TrackingPipe {
                file: File::from(tracked_read),
                reached_eof: Arc::clone(&reached_eof),
            }),
        };

        assert_eq!(
            collect_unix_output(
                Some(stdout),
                Some(stderr),
                &CaptureBudget {
                    limits: OutputLimits::default(),
                    aggregate: AtomicUsize::new(0),
                    exceeded: AtomicBool::new(false),
                    cancelled: AtomicBool::new(false),
                    notify: mpsc::channel().0,
                },
            )
            .unwrap_err()
            .to_string(),
            "injected output read failure"
        );
        assert!(reached_eof.load(Ordering::Relaxed));
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn unix_collector_notifies_waiter_after_read_failure() {
        let (failing_read, failing_write) = pipe_with(b"x");
        let (surviving_read, surviving_write) = pipe_with(b"");
        drop(failing_write);
        let stdout = ChildStdout {
            inner: Box::new(FailingPipe { descriptor: failing_read }),
        };
        let stderr = ChildStderr {
            inner: Box::new(File::from(surviving_read)),
        };
        let (collector, notification, budget) = spawn_output_collector(Some(stdout), Some(stderr), OutputLimits::default()).unwrap();

        notification.recv_timeout(Duration::from_secs(1)).unwrap();
        budget.cancel();
        drop(surviving_write);
        assert_eq!(
            join_output_collector(collector).unwrap_err().to_string(),
            "injected output read failure"
        );
    }

    #[test]
    fn output_collector_start_failure_is_reported_without_a_thread() {
        let (read, write) = pipe_with(&[]);
        drop(write);
        FAIL_OUTPUT_COLLECTOR_START.set(true);
        let stdout = ChildStdout {
            inner: Box::new(File::from(read)),
        };

        let error = spawn_output_collector(Some(stdout), None, OutputLimits::default()).unwrap_err();
        assert_eq!(error.error.to_string(), "injected output collector start failure");
        assert!(error.collector.is_none());
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn unix_collector_bounds_each_stream_and_aggregate() {
        let (stdout_read, stdout_write) = pipe_with(b"12345");
        let (stderr_read, stderr_write) = pipe_with(b"67890");
        drop(stdout_write);
        drop(stderr_write);
        let (notify, exceeded) = mpsc::channel();
        let error = collect_unix_output(
            Some(ChildStdout {
                inner: Box::new(File::from(stdout_read)),
            }),
            Some(ChildStderr {
                inner: Box::new(File::from(stderr_read)),
            }),
            &CaptureBudget {
                limits: OutputLimits::new(8, 9).unwrap(),
                aggregate: AtomicUsize::new(0),
                exceeded: AtomicBool::new(false),
                cancelled: AtomicBool::new(false),
                notify,
            },
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Other);
        exceeded.try_recv().unwrap();
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn cancelled_unix_collector_drops_pipe_without_waiting_for_eof() {
        let (read, _retained_write) = pipe_with(&[]);
        let (collector, _exceeded, budget) = spawn_output_collector(
            Some(ChildStdout {
                inner: Box::new(File::from(read)),
            }),
            None,
            OutputLimits::default(),
        )
        .unwrap();
        budget.cancel();
        assert_eq!(join_output_collector(collector).unwrap(), (Vec::new(), Vec::new()));
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn cancelled_unix_collector_stops_while_pipe_remains_readable() {
        let (read, write) = pipe_with(&[]);
        let writer = thread::spawn(move || {
            let mut file = File::from(write);
            let bytes = [b'x'; 4096];
            while file.write_all(&bytes).is_ok() {}
        });
        let (collector, _exceeded, budget) = spawn_output_collector(
            Some(ChildStdout {
                inner: Box::new(File::from(read)),
            }),
            None,
            OutputLimits::default(),
        )
        .unwrap();
        while budget.aggregate.load(Ordering::Acquire) == 0 {
            thread::yield_now();
        }
        budget.cancel();
        join_output_collector(collector).unwrap();
        writer.join().unwrap();
    }

    #[test]
    fn cleanup_failure_is_appended_without_hiding_primary_error() {
        let error = combine_output_errors(
            io::Error::other("output limit"),
            Some(io::Error::new(io::ErrorKind::TimedOut, "cleanup deadline")),
            Some(io::Error::other("collector panic")),
        );
        assert_eq!(error.kind(), io::ErrorKind::Other);
        assert_eq!(
            error.to_string(),
            "output limit; child cleanup failed: cleanup deadline; output collector shutdown failed: collector panic"
        );
    }
}
