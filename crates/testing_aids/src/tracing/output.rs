// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fs::{self, File};
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Once};

use ::tracing::Level;
use ::tracing::level_filters::LevelFilter;
use tracing_subscriber::Layer;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use crate::is_mutation_testing;

/// Enables logging of test output to the standard output stream.
///
/// Standard output is limited to INFO and above.
///
/// Logging is global state and will last until end of process - once you call this, all logging
/// statements in the process will be captured and be emitted to the standard output. This
/// is compatible with `write_to_stdout_and_file()`, as well - you can log to both standard
/// output and file at the same time and upgrade from one to the other at will.
///
/// All log entries will be written synchronously to ensure no data gets lost in case of
/// test failure or other anomaly.
///
/// Logging is disabled under mutation testing - this becomes a no-op.
pub fn write_to_stdout() {
    if is_mutation_testing() {
        // Under mutation testing, we do not log anything, to speed up the tests.
        return;
    }

    assert_initialized();
    STDOUT_ENABLED.store(true, Ordering::Relaxed);
}

/// Enables logging of test output to the standard output stream and to a specific log file.
///
/// Terminal output is limited to INFO and above, while the log file captures all messages.
///
/// The log file is saved in a "test-logs" directory under the Cargo workspace root.
///
/// Logging is global state and will last until end of process - once you call this, all logging
/// statements in the process will be captured and emitted to the standard output,
/// as well as logged to file. After the returned `FileGuard` is dropped, future log entries
/// will only go to the standard output.
///
/// All log entries will be written synchronously to ensure no data gets lost in case of
/// test failure or other anomaly.
///
/// Logging is disabled under mutation testing - this becomes a no-op.
pub fn write_to_stdout_and_file(file_name: &str) -> FileGuard {
    if is_mutation_testing() {
        // Under mutation testing, we do not log anything, to speed up the tests.
        return FileGuard::fake();
    }

    assert_initialized();
    STDOUT_ENABLED.store(true, Ordering::Relaxed);

    start_log_file_scope(file_name)
}

/// Looks upward in the filesystem from the current directory until it finds a directory with a
/// "Cargo.lock" file, indicating the root of a Cargo workspace. Returns the path to that directory.
fn workspace_directory() -> String {
    let mut dir = std::env::current_dir().unwrap();

    loop {
        if dir.join("Cargo.lock").exists() {
            return dir.to_string_lossy().to_string();
        }

        assert!(dir.pop(), "No Cargo workspace found in ancestors of the working directory");
    }
}

/// Returns the path to the directory where logs are written and ensures that this directory exists.
///
/// # Panics
///
/// Panics if the directory cannot be created or accessed.
#[must_use]
fn logs_directory() -> String {
    let workspace_dir = workspace_directory();
    let logs_dir = format!("{workspace_dir}/test-logs");

    if !std::path::Path::new(&logs_dir).exists() {
        fs::create_dir_all(&logs_dir).unwrap();
    }

    logs_dir
}

/// Returns the path to a specified log file within the directory where logs are written and ensures that this directory exists.
#[must_use]
pub fn log_file(file_name: &str) -> String {
    format!("{}/{}", logs_directory(), file_name)
}

/// Installs the process-global `tracing` subscriber, if not already installed,
/// with all output sinks silent.
///
/// The installed subscriber is permanently interested in every callsite at every
/// level (via an unfiltered buffer layer), so once it is present `tracing-core`
/// can never cache a callsite as disabled. It produces no output on
/// its own: stdout, file, and buffer sinks are all off until a `log_to_*` helper
/// turns one on.
///
/// Installing this from a `#[cfg(test)] #[ctor::ctor]` constructor guarantees the
/// fallback subscriber is present before any unit test runs, which keeps per-test
/// thread-local subscribers (`tracing::subscriber::set_default`) working
/// deterministically: a test can shadow the global default on its own thread and
/// the `DefaultGuard` restores the silent global fallback when dropped, and no
/// callsite is ever poisoned into the "disabled" state. See
/// `docs/tracing-tests.md`.
pub fn initialize() {
    ensure_initialized();
}

fn ensure_initialized() {
    INITIALIZER.call_once(|| {
        // Stdout output is gated by `STDOUT_ENABLED` (off by default) so the
        // fallback subscriber is silent until a `write_to_stdout*` helper opts in.
        let terminal_layer = tracing_subscriber::fmt::layer()
            .with_writer(StdoutWriterFactory)
            .with_filter(LevelFilter::from_level(Level::INFO));

        let file_layer = tracing_subscriber::fmt::layer()
            .with_writer(LogFileWriter)
            // No coloring codes or such fancy stuff in the file, please.
            .with_ansi(false);

        // The buffer layer carries no level filter, so it is interested in every
        // callsite at every level. This is deliberate and load-bearing: because
        // this always-interested layer is part of the process-global subscriber
        // from the very first initialization, `tracing-core` can never cache a
        // callsite's interest as "disabled". That is what makes both buffer capture
        // and per-test thread-local capture deterministic regardless of test
        // execution order. See `docs/tracing-tests.md`.
        let buffer_layer = tracing_subscriber::fmt::layer().with_writer(BufferWriterFactory).with_ansi(false);

        tracing_subscriber::registry()
            .with(terminal_layer)
            .with(file_layer)
            .with(buffer_layer)
            .try_init()
            .expect("this can only happen if something else besides testing_aids has configured logging");
    });
}

static INITIALIZER: Once = Once::new();

/// Asserts that [`initialize`] has already installed the silent always-interested
/// fallback subscriber.
///
/// The tracing helpers require that fallback to be present from process start - installed
/// in a `#[ctor::ctor]` constructor that runs before any test - so that no `tracing`
/// callsite can be poisoned into the "disabled" state before capture begins. Relying on a
/// helper to install it lazily would be too late: an earlier emission on a subscriber-less
/// thread could already have cached the callsite as disabled. See `docs/tracing-tests.md`.
///
/// # Panics
///
/// Panics if the fallback was never installed, which means the test binary is missing its
/// `#[ctor::ctor]` constructor calling [`initialize`].
pub(crate) fn assert_initialized() {
    assert!(
        INITIALIZER.is_completed(),
        "testing_aids tracing was used before initialize() ran; every test binary that \
         emits or inspects tracing must install the fallback from a `#[ctor::ctor]` constructor \
         calling `testing_aids::tracing::initialize()`. See docs/tracing-tests.md."
    );
}

/// Whether the terminal (stdout) sink of the global subscriber is active. Off by
/// default so [`initialize`] is silent; turned on by the `write_to_stdout*`
/// helpers.
static STDOUT_ENABLED: AtomicBool = AtomicBool::new(false);

/// Produces writers to stdout when stdout logging is enabled, discarding otherwise.
#[derive(Debug)]
struct StdoutWriterFactory;

impl<'a> MakeWriter<'a> for StdoutWriterFactory {
    type Writer = StdoutWriter;

    fn make_writer(&'a self) -> Self::Writer {
        StdoutWriter {
            enabled: STDOUT_ENABLED.load(Ordering::Relaxed),
        }
    }
}

/// Writes to stdout when enabled, or discards output otherwise.
#[derive(Debug)]
struct StdoutWriter {
    enabled: bool,
}

impl Write for StdoutWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.enabled {
            std::io::stdout().write_all(buf)?;
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if self.enabled {
            std::io::stdout().flush()?;
        }
        Ok(())
    }
}

/// Enables logging of test output to the standard output stream and captures every
/// emitted log line into an in-memory buffer for the lifetime of the returned guard.
///
/// Terminal output is limited to INFO and above, while the buffer captures all
/// messages regardless of level.
///
/// This is the sanctioned way to assert on `tracing` output in tests. It routes
/// through the single process-global subscriber (installed by the test binary's
/// `#[ctor::ctor]` constructor and left permanently interested), so a prior
/// emission on a subscriber-less thread can never suppress later capture. See
/// [`docs/tracing-tests.md`] for the full rationale and rules.
///
/// Capture is process-global: only one buffer can be active at a time. Every test
/// in an integration-test binary that captures `tracing` output MUST therefore be
/// annotated `#[serial]` so the tests are mutually exclusive.
///
/// Retrieve the captured lines with [`BufferGuard::into_inner`].
///
/// # Panics
///
/// Panics if a buffer is already active (i.e. two capturing tests ran
/// concurrently because one of them was missing `#[serial]`), or if the fallback
/// subscriber was never installed (the binary is missing its `#[ctor::ctor]`
/// constructor calling [`initialize`]).
///
/// [`docs/tracing-tests.md`]: https://github.com/microsoft/oxidizer/blob/main/docs/tracing-tests.md
pub fn write_to_stdout_and_buffer() -> BufferGuard {
    assert_initialized();
    STDOUT_ENABLED.store(true, Ordering::Relaxed);

    let buffer = Arc::new(Mutex::new(Vec::<String>::new()));

    {
        let mut slot = LOG_BUFFER.lock().unwrap();
        assert!(
            slot.is_none(),
            "a log buffer is already active; tracing capture is process-global, so every test in a \
             binary that captures tracing output must be annotated `#[serial]`"
        );
        *slot = Some(Arc::clone(&buffer));
    }

    BufferGuard { buffer: Some(buffer) }
}

/// The buffer that captured log lines are written to while a [`BufferGuard`] is
/// active, one entry per line. `None` when no capture is in progress.
static LOG_BUFFER: Mutex<Option<Arc<Mutex<Vec<String>>>>> = Mutex::new(None);

/// Scopes an in-memory `tracing` capture started by [`write_to_stdout_and_buffer`].
///
/// While this guard is alive, every emitted log line is appended to an in-memory
/// buffer. Consume the guard with [`into_inner`](Self::into_inner) to detach the
/// buffer and obtain the captured lines. Dropping the guard without consuming it
/// simply detaches the buffer and discards its contents.
#[derive(Debug)]
#[must_use]
pub struct BufferGuard {
    // `Some` until consumed by `into_inner`; `None` afterwards so `Drop` is a no-op.
    buffer: Option<Arc<Mutex<Vec<String>>>>,
}

impl BufferGuard {
    /// Returns a snapshot of everything logged so far during the guard's lifetime,
    /// one entry per emitted log line, without detaching the capture buffer.
    ///
    /// Use this to poll for an event that is emitted asynchronously (for example,
    /// on a background thread) while the guard remains active. Call
    /// [`into_inner`](Self::into_inner) once at the end to detach and finish.
    ///
    /// # Panics
    ///
    /// Panics if the buffer mutex is poisoned, or if called after
    /// [`into_inner`](Self::into_inner) has consumed the guard.
    #[must_use]
    pub fn snapshot(&self) -> Vec<String> {
        let buffer = self.buffer.as_ref().expect("buffer is present until into_inner consumes the guard");
        buffer.lock().unwrap().clone()
    }

    /// Detaches the capture buffer and returns everything logged during the guard's
    /// lifetime, one entry per emitted log line.
    ///
    /// `tracing`'s formatting layers write synchronously on each event, so there is
    /// no asynchronous buffering to flush; the returned lines are complete as of the
    /// last emission on this thread before the call.
    ///
    /// # Panics
    ///
    /// Panics if the buffer mutex is poisoned.
    #[must_use]
    pub fn into_inner(mut self) -> Vec<String> {
        let buffer = self.buffer.take().expect("buffer is present until into_inner consumes the guard");

        // Detach global capture so the next test starts clean.
        *LOG_BUFFER.lock().unwrap() = None;

        std::mem::take(&mut *buffer.lock().unwrap())
    }
}

impl Drop for BufferGuard {
    fn drop(&mut self) {
        if self.buffer.is_some() {
            // Guard dropped without `into_inner`: detach so capture does not leak
            // into a subsequent test.
            *LOG_BUFFER.lock().unwrap() = None;
        }
    }
}

/// Produces writers that append to the currently-active capture buffer, if any.
#[derive(Debug)]
struct BufferWriterFactory;

impl<'a> MakeWriter<'a> for BufferWriterFactory {
    type Writer = BufferWriter;

    fn make_writer(&'a self) -> Self::Writer {
        let slot = LOG_BUFFER.lock().unwrap();
        BufferWriter {
            buffer: slot.as_ref().map(Arc::clone),
            pending: Vec::new(),
        }
    }
}

/// Accumulates the bytes of a single formatted event and, on flush or drop, commits
/// each complete newline-delimited line to the active capture buffer as its own entry.
///
/// `tracing_subscriber`'s `fmt` layer creates one writer per event but may call
/// [`write`](Write::write) more than once, so a single `write` is not a line. Buffering
/// per writer and splitting on newlines keeps each captured entry a whole line and keeps
/// concurrently-emitted events from interleaving at byte granularity in the shared buffer.
#[derive(Debug)]
struct BufferWriter {
    buffer: Option<Arc<Mutex<Vec<String>>>>,
    pending: Vec<u8>,
}

impl BufferWriter {
    /// Moves every complete line out of `pending` into the shared buffer, leaving any
    /// trailing bytes that are not yet newline-terminated in `pending`.
    fn commit(&mut self) {
        let Some(buffer) = &self.buffer else {
            self.pending.clear();
            return;
        };

        let Some(last_newline) = self.pending.iter().rposition(|&b| b == b'\n') else {
            return;
        };

        let complete = &self.pending[..=last_newline];
        let lines: Vec<String> = String::from_utf8_lossy(complete).lines().map(str::to_string).collect();
        buffer.lock().unwrap().extend(lines);
        self.pending.drain(..=last_newline);
    }
}

impl Write for BufferWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.buffer.is_some() {
            self.pending.extend_from_slice(buf);
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.commit();
        Ok(())
    }
}

impl Drop for BufferWriter {
    fn drop(&mut self) {
        self.commit();
    }
}

/// We write log entries to this file (if not `None`).
static LOG_FILE: Mutex<Option<File>> = Mutex::new(None);

/// Defines the scope within which all log entries (from all threads) go to a log file.
///
/// Once this scope ends, new log entries will no longer go to a file.
#[derive(Debug)]
#[must_use]
pub struct FileGuard {
    // Under mutation testing, we pretend to log but do not actually do so, to speed up the tests.
    fake: bool,
}

impl FileGuard {
    const fn new() -> Self {
        Self { fake: false }
    }

    const fn fake() -> Self {
        Self { fake: true }
    }
}

impl Drop for FileGuard {
    fn drop(&mut self) {
        if self.fake {
            return;
        }

        let mut log_file = LOG_FILE.lock().unwrap();

        assert!(log_file.is_some());
        *log_file = None;
    }
}

fn start_log_file_scope(file_name: &str) -> FileGuard {
    let file = File::create(log_file(file_name)).unwrap();

    {
        let mut log_file = LOG_FILE.lock().unwrap();

        assert!(
            log_file.is_none(),
            "a log file is already active; multiple tests cannot log to file in parallel within the same process - logging is global state"
        );

        *log_file = Some(file);
    }

    FileGuard::new()
}

#[derive(Debug)]
struct LogFileWriter;

impl<'a> MakeWriter<'a> for LogFileWriter {
    type Writer = LogWriter;

    fn make_writer(&'a self) -> Self::Writer {
        let log_file = LOG_FILE.lock().unwrap();

        LogWriter {
            file: (*log_file).as_ref().map(|f| f.try_clone().unwrap()),
        }
    }
}

/// Could either wrap a `File` or be a no-op placeholder (used when no log file is configured).
#[derive(Debug)]
struct LogWriter {
    file: Option<File>,
}

impl Write for LogWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.file.as_mut().map_or(Ok(buf.len()), |file| file.write(buf))
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.file.as_mut().map_or(Ok(()), File::flush)
    }
}
