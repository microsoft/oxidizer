// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fs::{self, File};
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Once};

use tracing::Level;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::fmt::writer::MakeWriterExt;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use super::format::formatting_layer;
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
/// Output is disabled under mutation testing - this becomes a no-op. The initialized
/// subscriber still evaluates fields and formats events and spans synchronously.
///
/// # Panics
///
/// Outside mutation testing, panics if `testing_aids::init_tracing!()` was never
/// invoked, which means the test binary is missing its `#[ctor::ctor]`
/// process-initialization function. See `docs/tracing-tests.md`.
pub fn write_to_stdout() {
    let mutation_testing = is_mutation_testing();
    if mutation_testing {
        // Suppress diagnostic output, but leave the initialized formatter enabled.
        return;
    }

    assert_initialized();
    enable_stdout(&STDOUT_ENABLED, mutation_testing);
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
/// Output is disabled under mutation testing - this becomes a no-op. The initialized
/// subscriber still evaluates fields and formats events and spans synchronously.
///
/// # Panics
///
/// Outside mutation testing, panics if a log file is already active (another test
/// is logging to file in the same process without `#[serial]`), or if
/// `testing_aids::init_tracing!()` was never invoked, which means the test binary
/// is missing its `#[ctor::ctor]` process-initialization function. See
/// `docs/tracing-tests.md`.
pub fn write_to_stdout_and_file(file_name: &str) -> FileGuard {
    let mutation_testing = is_mutation_testing();
    if mutation_testing {
        // Suppress diagnostic output, but leave the initialized formatter enabled.
        return FileGuard::fake();
    }

    assert_initialized();
    enable_stdout(&STDOUT_ENABLED, mutation_testing);

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
/// level, so once it is present `tracing-core` can never cache a callsite as disabled.
/// Field expressions and `Debug`, `Display`, and message formatting are exercised
/// synchronously, including initial and recorded span fields and during mutation
/// testing. Output is discarded after formatting until a `write_to_*` helper enables
/// a sink. Events are formatted once, regardless of the number of active sinks.
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
        // Filter writers, not the layer: interest and formatting must remain enabled
        // even when no sink wants output. Tee the rendered bytes to avoid invoking
        // user formatters again for each sink.
        let writer = stdout_writer(std::io::stdout, || STDOUT_ENABLED.load(Ordering::Relaxed))
            .and(LogFileWriter.with_filter(|_| file_active()))
            .and(BufferWriterFactory.with_filter(|_| buffer_active()));

        tracing_subscriber::registry()
            .with(formatting_layer(writer))
            .try_init()
            .expect("this can only happen if something else besides testing_aids has configured logging");
    });
}

/// Whether the file sink is currently active.
///
/// Reads a lock-free flag rather than locking [`LOG_FILE`] so the writer filter
/// stays off the global mutex on the hot path. The flag is kept in sync with
/// `LOG_FILE` by setting it under that lock whenever the file sink is attached or
/// detached.
fn file_active() -> bool {
    FILE_ENABLED.load(Ordering::Acquire)
}

/// Whether the buffer sink is currently active.
///
/// Reads a lock-free flag rather than locking [`LOG_BUFFER`] so the writer filter
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
         depends on tracing directly or transitively must install the subscriber from a `#[ctor::ctor]` \
         process-initialization function calling `testing_aids::init_tracing!()`. See \
         docs/tracing-tests.md."
    );
}

/// Whether the terminal (stdout) sink of the global subscriber is active. Off by
/// default so [`initialize`] is silent; turned on by the `write_to_stdout*`
/// helpers.
static STDOUT_ENABLED: AtomicBool = AtomicBool::new(false);

fn enable_stdout(enabled: &AtomicBool, mutation_testing: bool) {
    if !mutation_testing {
        enabled.store(true, Ordering::Relaxed);
    }
}

fn stdout_writer<W, E>(writer: W, enabled: E) -> impl for<'a> MakeWriter<'a>
where
    W: for<'a> MakeWriter<'a>,
    E: Fn() -> bool,
{
    writer.with_max_level(Level::INFO).with_filter(move |_| enabled())
}

/// Enables logging of test output to the standard output stream and captures every
/// emitted log line into an in-memory buffer for the lifetime of the returned guard.
///
/// Terminal output is limited to INFO and above, while the buffer captures all
/// messages regardless of level.
///
/// Only buffer capture is scoped to the returned guard. As with [`write_to_stdout`],
/// the stdout sink stays enabled for the remainder of the process once this is called;
/// dropping the guard detaches the buffer but does not silence stdout.
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
    enable_stdout(&STDOUT_ENABLED, is_mutation_testing());

    let buffer = Arc::new(Mutex::new(Vec::<String>::new()));

    // Determine whether a buffer is already active and, if not, install ours - all while
    // holding the lock - but release the lock *before* asserting, so a failed assertion
    // (a `#[serial]` violation) never panics with the lock held and thus never poisons
    // `LOG_BUFFER`. This keeps the `LOG_BUFFER_NEVER_POISONED` justification below honest.
    let already_active = {
        let mut slot = LOG_BUFFER.lock().expect(LOG_BUFFER_NEVER_POISONED);
        let occupied = slot.is_some();
        if !occupied {
            *slot = Some(Arc::clone(&buffer));
            BUFFER_ENABLED.store(true, Ordering::Release);
        }
        occupied
    };
    assert!(
        !already_active,
        "a log buffer is already active; tracing capture is process-global and records events \
         from any thread, so if a binary uses this helper then every test in that binary - not \
         just the capturing ones - must be annotated `#[serial]`"
    );

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
    /// `tracing`'s formatting layer writes synchronously on each event, so there is
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

/// Justification for `expect` on the [`LOG_FILE`] mutex: while it is held, the only
/// operations performed are reading or swapping its `Option` and fallible I/O
/// (`File::create`, `File::try_clone`) whose `Result` is captured but never unwrapped
/// until after the lock is released. None of these can panic with the lock held, so the
/// mutex can never be poisoned.
const LOG_FILE_NEVER_POISONED: &str =
    "LOG_FILE is only locked for Option access and non-panicking fallible I/O (unwrapped after release), which cannot poison the mutex";

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

        // Clear the slot under the lock, then assert *after* releasing it so a failed
        // assertion never panics with the lock held and thus never poisons `LOG_FILE`.
        let was_active = {
            let mut log_file = LOG_FILE.lock().expect(LOG_FILE_NEVER_POISONED);
            let occupied = log_file.is_some();
            *log_file = None;
            FILE_ENABLED.store(false, Ordering::Release);
            occupied
        };

        assert!(was_active, "a log file scope ended without an active log file");
    }
}

fn start_log_file_scope(file_name: &str) -> FileGuard {
    let path = log_file(file_name);

    // Check occupancy and, only if the slot is vacant, create the file and install it -
    // all under a single lock so the check and install are atomic (a second concurrent
    // scope observes the first's file and creates nothing, rather than both truncating and
    // overwriting each other). Creating the file only when vacant also ensures a
    // `#[serial]` violation never truncates the file the active scope is writing to. The
    // fallible `File::create` result is captured but *not* unwrapped under the lock, and the
    // assertion is deferred until after the lock is released, so neither an I/O failure nor
    // a failed assertion can ever panic while the lock is held and thus poison `LOG_FILE`.
    let (already_active, create_result) = {
        let mut log_file = LOG_FILE.lock().expect(LOG_FILE_NEVER_POISONED);
        if log_file.is_some() {
            (true, None)
        } else {
            match File::create(path) {
                Ok(file) => {
                    *log_file = Some(file);
                    FILE_ENABLED.store(true, Ordering::Release);
                    (false, Some(Ok(())))
                }
                Err(error) => (false, Some(Err(error))),
            }
        }
    };
    assert!(
        !already_active,
        "a log file is already active; multiple tests cannot log to file in parallel within the same process - logging is global state"
    );
    // Surface any `File::create` failure now that the lock is released.
    if let Some(result) = create_result {
        result.unwrap();
    }

    FileGuard::new()
}

#[derive(Debug)]
struct LogFileWriter;

impl<'a> MakeWriter<'a> for LogFileWriter {
    type Writer = LogWriter;

    fn make_writer(&'a self) -> Self::Writer {
        // Clone the file handle under the lock but unwrap the clone result *after*
        // releasing it, so a `try_clone` failure never panics with the lock held and thus
        // never poisons `LOG_FILE`.
        let cloned = {
            let log_file = LOG_FILE.lock().expect(LOG_FILE_NEVER_POISONED);
            (*log_file).as_ref().map(File::try_clone)
        };

        LogWriter {
            file: cloned.map(|result| result.unwrap()),
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

    #[test]
    fn stdout_route_filters_levels_after_activation() {
        assert_stdout_route(
            false,
            &[
                "ERROR stdout-routing-test: after-ERROR",
                " WARN stdout-routing-test: after-WARN",
                " INFO stdout-routing-test: after-INFO",
            ],
        );
    }

    #[test]
    fn stdout_route_stays_silent_during_mutation_testing() {
        assert_stdout_route(true, &[]);
    }

    fn assert_stdout_route(mutation_testing: bool, expected: &[&str]) {
        let enabled = Arc::new(AtomicBool::new(false));
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let writer = stdout_writer(
            {
                let buffer = Arc::clone(&buffer);
                move || BufferWriter {
                    buffer: Some(Arc::clone(&buffer)),
                    pending: Vec::new(),
                }
            },
            {
                let enabled = Arc::clone(&enabled);
                move || enabled.load(Ordering::Relaxed)
            },
        );
        let _guard = tracing_subscriber::registry().with(formatting_layer(writer)).set_default();

        emit_levels("before");
        assert_eq!(*buffer.lock().unwrap(), Vec::<String>::new());
        enable_stdout(&enabled, mutation_testing);
        emit_levels("after");
        assert_eq!(*buffer.lock().unwrap(), expected);
    }

    fn emit_levels(phase: &str) {
        tracing::error!(target: "stdout-routing-test", "{phase}-ERROR");
        tracing::warn!(target: "stdout-routing-test", "{phase}-WARN");
        tracing::info!(target: "stdout-routing-test", "{phase}-INFO");
        tracing::debug!(target: "stdout-routing-test", "{phase}-DEBUG");
        tracing::trace!(target: "stdout-routing-test", "{phase}-TRACE");
    }

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
