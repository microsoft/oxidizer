// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fs::{self, File};
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Once};

use tracing::subscriber::Interest;
use tracing::{Level, Metadata, Subscriber};
use tracing_subscriber::Layer;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::{Context, Filter, SubscriberExt};
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
///
/// # Panics
///
/// Panics if `testing_aids::init_tracing!()` was never invoked, which means the
/// test binary is missing its `#[ctor::ctor]` process-initialization function. See
/// `docs/tracing-tests.md`.
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
///
/// # Panics
///
/// Panics if a log file is already active (another test is logging to file in the same
/// process without `#[serial]`), or if `testing_aids::init_tracing!()` was never
/// invoked, which means the test binary is missing its `#[ctor::ctor]`
/// process-initialization function. See
/// `docs/tracing-tests.md`.
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
/// level (via an always-interested no-op interest-keeper layer), so once it is
/// present `tracing-core` can never cache a callsite as disabled and event field
/// expressions are always evaluated. It produces no output on its own: the stdout,
/// file, and buffer sinks are separate formatting layers, each off (and skipping
/// formatting) until a `write_to_*` helper turns it on.
///
/// Installing this from a `#[cfg(test)] #[ctor::ctor]` process-initialization
/// function guarantees the subscriber is present before any unit test runs, which
/// keeps per-test thread-local subscribers (`tracing::subscriber::set_default`)
/// working deterministically: a test can shadow the global default on its own thread
/// and the `DefaultGuard` restores the silent global subscriber when dropped, and no
/// callsite is ever poisoned into the "disabled" state. See
/// `docs/tracing-tests.md`.
///
/// # Panics
///
/// Panics if another global `tracing` subscriber has already been installed by
/// something other than `testing_aids` (for example a test that called
/// `tracing::subscriber::set_global_default`). See `docs/tracing-tests.md`.
pub fn initialize() {
    ensure_initialized();
}

fn ensure_initialized() {
    INITIALIZER.call_once(|| {
        // Each output sink is a formatting layer gated by a `SinkFilter` that is
        // active only while that sink is turned on. The filter runs before the
        // layer formats an event, so when a sink is off its (relatively expensive)
        // event formatting is skipped entirely rather than being rendered and then
        // discarded by the writer.
        // All layers omit the timestamp. Wall-clock timestamps would make captured
        // output non-deterministic (breaking buffer assertions) and reading the clock
        // via `SystemTime::now` is an unsupported operation under Miri's isolation.
        let terminal_layer = tracing_subscriber::fmt::layer()
            .without_time()
            .with_writer(StdoutWriterFactory)
            .with_filter(SinkFilter {
                active: stdout_active,
                max_level: Some(Level::INFO),
            });

        let file_layer = tracing_subscriber::fmt::layer()
            .without_time()
            .with_writer(LogFileWriter)
            // No coloring codes or such fancy stuff in the file, please.
            .with_ansi(false)
            .with_filter(SinkFilter {
                active: file_active,
                max_level: None,
            });

        let buffer_layer = tracing_subscriber::fmt::layer()
            .without_time()
            .with_writer(BufferWriterFactory)
            .with_ansi(false)
            .with_filter(SinkFilter {
                active: buffer_active,
                max_level: None,
            });

        // The interest keeper carries no filter and does no formatting, so it is
        // permanently interested in every callsite at every level. This is
        // deliberate and load-bearing on two counts. First, because this
        // always-interested layer is part of the process-global subscriber from the
        // very first initialization, `tracing-core` can never cache a callsite's
        // interest as "disabled", which makes both buffer capture and per-test
        // thread-local capture deterministic regardless of test execution order.
        // Second, keeping every callsite enabled forces `tracing` to evaluate event
        // field expressions (e.g. `duration.as_nanos()`) on every emission even when
        // no sink is capturing, so coverage of those expressions is deterministic.
        // See `docs/tracing-tests.md`.
        tracing_subscriber::registry()
            .with(InterestKeeper)
            .with(terminal_layer)
            .with(file_layer)
            .with(buffer_layer)
            .try_init()
            .expect("this can only happen if something else besides testing_aids has configured logging");
    });
}

/// No-op layer that is permanently interested in every callsite at every level.
///
/// It performs no formatting and produces no output; its sole job is to keep every
/// `tracing` callsite enabled from process start. This prevents `tracing-core` from
/// caching a callsite's interest as "disabled" and forces event field expressions to
/// be evaluated on every emission, which keeps capture and coverage deterministic
/// independent of which output sinks are active. See `docs/tracing-tests.md`.
#[derive(Debug)]
struct InterestKeeper;

impl<S: Subscriber> Layer<S> for InterestKeeper {
    fn register_callsite(&self, _metadata: &'static Metadata<'static>) -> Interest {
        Interest::always()
    }
}

/// Per-layer filter that enables its formatting layer only while the associated sink
/// is active, so event formatting is skipped when nothing is capturing.
///
/// Returns [`Interest::sometimes`] so the decision is re-evaluated on every event
/// rather than cached: the sink activation flags change at runtime, and the always
/// interested [`InterestKeeper`] guarantees no callsite is ever cached as disabled.
struct SinkFilter {
    /// Whether this sink is currently active.
    active: fn() -> bool,
    /// The most verbose level this sink accepts, or `None` for every level.
    max_level: Option<Level>,
}

impl<S> Filter<S> for SinkFilter {
    fn enabled(&self, meta: &Metadata<'_>, _cx: &Context<'_, S>) -> bool {
        (self.active)() && self.max_level.is_none_or(|level| *meta.level() <= level)
    }

    fn callsite_enabled(&self, _metadata: &'static Metadata<'static>) -> Interest {
        Interest::sometimes()
    }
}

/// Whether the stdout sink is currently active.
fn stdout_active() -> bool {
    STDOUT_ENABLED.load(Ordering::Relaxed)
}

/// Whether the file sink is currently active.
///
/// Reads a lock-free flag rather than locking [`LOG_FILE`] so the per-event filter
/// stays off the global mutex on the hot path. The flag is kept in sync with
/// `LOG_FILE` by setting it under that lock whenever the file sink is attached or
/// detached.
fn file_active() -> bool {
    FILE_ENABLED.load(Ordering::Acquire)
}

/// Whether the buffer sink is currently active.
///
/// Reads a lock-free flag rather than locking [`LOG_BUFFER`] so the per-event filter
/// stays off the global mutex on the hot path. The flag is kept in sync with
/// `LOG_BUFFER` by setting it under that lock whenever the buffer sink is attached or
/// detached.
fn buffer_active() -> bool {
    BUFFER_ENABLED.load(Ordering::Acquire)
}

static INITIALIZER: Once = Once::new();

/// Asserts that [`initialize`] has already installed the silent always-interested
/// subscriber.
///
/// The tracing helpers require that subscriber to be present from process start -
/// installed in a `#[ctor::ctor]` process-initialization function that runs before any
/// test - so that no `tracing` callsite can be poisoned into the "disabled" state before
/// capture begins. Relying on a helper to install it lazily would be too late: an earlier
/// emission on a subscriber-less thread could already have cached the callsite as
/// disabled. See `docs/tracing-tests.md`.
///
/// # Panics
///
/// Panics if the subscriber was never installed, which means the test binary is missing
/// its `#[ctor::ctor]` process-initialization function calling [`initialize`].
pub(crate) fn assert_initialized() {
    assert!(
        INITIALIZER.is_completed(),
        "testing_aids tracing was used before initialize() ran; every test binary that \
         emits or inspects tracing must install the subscriber from a `#[ctor::ctor]` \
         process-initialization function calling `testing_aids::init_tracing!()`. See \
         docs/tracing-tests.md."
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
/// `#[ctor::ctor]` process-initialization function and left permanently interested), so
/// a prior emission on a subscriber-less thread can never suppress later capture. See
/// [`docs/tracing-tests.md`] for the full rationale and rules.
///
/// Capture is process-global: only one buffer can be active at a time, and the
/// buffer records events from *any* thread, including tests that never call this
/// helper. Therefore, if a test binary uses this helper at all, EVERY test in that
/// binary MUST be annotated `#[serial]` - not just the capturing ones - so that no
/// other test runs concurrently and emits into the shared buffer.
///
/// Retrieve the captured lines with [`BufferGuard::into_inner`].
///
/// # Panics
///
/// Panics if a buffer is already active (i.e. another test ran concurrently because
/// some test in the binary was missing `#[serial]`), or if
/// `testing_aids::init_tracing!()` was never invoked (the binary is missing its
/// `#[ctor::ctor]` process-initialization function).
///
/// [`docs/tracing-tests.md`]: https://github.com/microsoft/oxidizer/blob/main/docs/tracing-tests.md
pub fn write_to_stdout_and_buffer() -> BufferGuard {
    assert_initialized();

    // Unlike `write_to_stdout`/`write_to_stdout_and_file`, buffer capture is not
    // disabled under mutation testing: its contents are asserted upon by the calling
    // test, so a disabled buffer would break those tests rather than merely omit
    // diagnostic output. The stdout tee, however, is purely diagnostic, so we silence
    // it under mutation testing to avoid the output overhead.
    if !is_mutation_testing() {
        STDOUT_ENABLED.store(true, Ordering::Relaxed);
    }

    let buffer = Arc::new(Mutex::new(Vec::<String>::new()));

    {
        let mut slot = LOG_BUFFER.lock().expect(LOG_BUFFER_NEVER_POISONED);
        assert!(
            slot.is_none(),
            "a log buffer is already active; tracing capture is process-global and records events \
             from any thread, so if a binary uses this helper then every test in that binary - not \
             just the capturing ones - must be annotated `#[serial]`"
        );
        *slot = Some(Arc::clone(&buffer));
        BUFFER_ENABLED.store(true, Ordering::Release);
    }

    BufferGuard { buffer: Some(buffer) }
}

/// Justification for `expect` on the [`LOG_BUFFER`] mutex: it is only ever locked to
/// read or swap its `Option`, operations that cannot panic, so it can never be poisoned.
const LOG_BUFFER_NEVER_POISONED: &str = "LOG_BUFFER is only locked to swap its Option, which cannot panic, so the mutex is never poisoned";

/// Justification for `expect` on a capture buffer mutex: it is only ever locked for
/// infallible `Vec` operations, so it can never be poisoned.
const CAPTURE_BUFFER_NEVER_POISONED: &str =
    "the capture buffer is only locked for infallible Vec operations, so the mutex is never poisoned";

/// The buffer that captured log lines are written to while a [`BufferGuard`] is
/// active, one entry per line. `None` when no capture is in progress.
static LOG_BUFFER: Mutex<Option<Arc<Mutex<Vec<String>>>>> = Mutex::new(None);

/// Lock-free mirror of whether [`LOG_BUFFER`] currently holds a buffer. Set under the
/// `LOG_BUFFER` lock on attach/detach and read by [`buffer_active`] on the per-event
/// path so the filter never has to take the global mutex.
static BUFFER_ENABLED: AtomicBool = AtomicBool::new(false);

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
    #[must_use]
    #[expect(
        clippy::missing_panics_doc,
        reason = "the only expects are on an internal mutex that is never poisoned and on an \
                  Option that is Some until into_inner consumes the guard; neither can fail"
    )]
    pub fn snapshot(&self) -> Vec<String> {
        let buffer = self.buffer.as_ref().expect("buffer is present until into_inner consumes the guard");
        buffer.lock().expect(CAPTURE_BUFFER_NEVER_POISONED).clone()
    }

    /// Detaches the capture buffer and returns everything logged during the guard's
    /// lifetime, one entry per emitted log line.
    ///
    /// `tracing`'s formatting layers write synchronously on each event, so there is
    /// no asynchronous buffering to flush; the returned lines are complete as of the
    /// last emission on this thread before the call.
    #[must_use]
    #[expect(
        clippy::missing_panics_doc,
        reason = "the only expects are on internal mutexes that are never poisoned and on an \
                  Option that is Some until into_inner consumes the guard; neither can fail"
    )]
    pub fn into_inner(mut self) -> Vec<String> {
        let buffer = self.buffer.take().expect("buffer is present until into_inner consumes the guard");

        // Detach global capture so the next test starts clean.
        {
            let mut slot = LOG_BUFFER.lock().expect(LOG_BUFFER_NEVER_POISONED);
            *slot = None;
            BUFFER_ENABLED.store(false, Ordering::Release);
        }

        std::mem::take(&mut *buffer.lock().expect(CAPTURE_BUFFER_NEVER_POISONED))
    }
}

impl Drop for BufferGuard {
    fn drop(&mut self) {
        if self.buffer.is_some() {
            // Guard dropped without `into_inner`: detach so capture does not leak
            // into a subsequent test.
            let mut slot = LOG_BUFFER.lock().expect(LOG_BUFFER_NEVER_POISONED);
            *slot = None;
            BUFFER_ENABLED.store(false, Ordering::Release);
        }
    }
}

/// Produces writers that append to the currently-active capture buffer, if any.
#[derive(Debug)]
struct BufferWriterFactory;

impl<'a> MakeWriter<'a> for BufferWriterFactory {
    type Writer = BufferWriter;

    fn make_writer(&'a self) -> Self::Writer {
        let slot = LOG_BUFFER.lock().expect(LOG_BUFFER_NEVER_POISONED);
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
        buffer.lock().expect(CAPTURE_BUFFER_NEVER_POISONED).extend(lines);
        self.pending.drain(..=last_newline);
    }

    /// Commits every complete line and then any remaining trailing bytes as a final
    /// line, even if they are not newline-terminated. Used on drop so a formatter that
    /// does not newline-terminate its last write cannot silently lose that line.
    fn commit_final(&mut self) {
        self.commit();

        let Some(buffer) = &self.buffer else {
            return;
        };

        if self.pending.is_empty() {
            return;
        }

        let line = String::from_utf8_lossy(&self.pending).into_owned();
        buffer.lock().expect(CAPTURE_BUFFER_NEVER_POISONED).push(line);
        self.pending.clear();
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
        self.commit_final();
    }
}

/// We write log entries to this file (if not `None`).
static LOG_FILE: Mutex<Option<File>> = Mutex::new(None);

/// Lock-free mirror of whether [`LOG_FILE`] currently holds a file. Set under the
/// `LOG_FILE` lock on attach/detach and read by [`file_active`] on the per-event path
/// so the filter never has to take the global mutex.
static FILE_ENABLED: AtomicBool = AtomicBool::new(false);

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
        FILE_ENABLED.store(false, Ordering::Release);
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
        FILE_ENABLED.store(true, Ordering::Release);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn new_writer() -> (BufferWriter, Arc<Mutex<Vec<String>>>) {
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let writer = BufferWriter {
            buffer: Some(Arc::clone(&buffer)),
            pending: Vec::new(),
        };
        (writer, buffer)
    }

    #[test]
    fn write_splits_on_newlines_into_separate_entries() {
        let (mut writer, buffer) = new_writer();

        writer.write_all(b"first line\nsecond line\n").unwrap();
        writer.flush().unwrap();

        assert_eq!(*buffer.lock().unwrap(), vec!["first line", "second line"]);
    }

    #[test]
    fn write_does_not_split_a_line_across_multiple_writes() {
        let (mut writer, buffer) = new_writer();

        // A single logical line delivered in several `write` calls must become one entry.
        writer.write_all(b"one ").unwrap();
        writer.write_all(b"logical ").unwrap();
        writer.write_all(b"line\n").unwrap();
        writer.flush().unwrap();

        assert_eq!(*buffer.lock().unwrap(), vec!["one logical line"]);
    }

    #[test]
    fn flush_retains_a_trailing_partial_line_until_terminated() {
        let (mut writer, buffer) = new_writer();

        writer.write_all(b"complete\npartial").unwrap();
        writer.flush().unwrap();

        // Only the newline-terminated line is committed; the partial line waits.
        assert_eq!(*buffer.lock().unwrap(), vec!["complete"]);
    }

    #[test]
    fn drop_flushes_a_final_line_without_a_trailing_newline() {
        let (mut writer, buffer) = new_writer();

        writer.write_all(b"no trailing newline").unwrap();
        drop(writer);

        assert_eq!(*buffer.lock().unwrap(), vec!["no trailing newline"]);
    }
}
