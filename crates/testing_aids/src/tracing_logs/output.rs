// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
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
    write_to_stdout_mode(is_mutation_testing());
}

fn write_to_stdout_mode(mutation_testing: bool) {
    write_to_stdout_initialized(mutation_testing, &INITIALIZER);
}

fn write_to_stdout_initialized(mutation_testing: bool, initializer: &Once) {
    if mutation_testing {
        // Suppress diagnostic output, but leave the initialized formatter enabled.
        return;
    }

    assert_initialized_with(initializer);
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
    write_to_stdout_and_file_mode(file_name, is_mutation_testing())
}

fn write_to_stdout_and_file_mode(file_name: &str, mutation_testing: bool) -> FileGuard {
    write_to_stdout_and_file_initialized(file_name, mutation_testing, &INITIALIZER)
}

fn write_to_stdout_and_file_initialized(file_name: &str, mutation_testing: bool, initializer: &Once) -> FileGuard {
    if mutation_testing {
        // Suppress diagnostic output, but leave the initialized formatter enabled.
        return FileGuard::fake();
    }

    assert_initialized_with(initializer);
    enable_stdout(&STDOUT_ENABLED, mutation_testing);

    start_log_file_scope(file_name)
}

/// Looks upward in the filesystem from the current directory until it finds a directory with a
/// "Cargo.lock" file, indicating the root of a Cargo workspace. Returns the path to that directory.
fn workspace_directory() -> String {
    find_workspace_directory(std::env::current_dir().unwrap(), Path::exists)
}

fn find_workspace_directory(mut dir: PathBuf, exists: impl Fn(&Path) -> bool) -> String {
    loop {
        if exists(&dir.join("Cargo.lock")) {
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
    ensure_logs_directory(&workspace_dir, Path::exists, |path| fs::create_dir_all(path))
}

fn ensure_logs_directory(workspace_dir: &str, exists: impl FnOnce(&Path) -> bool, create: impl FnOnce(&Path) -> io::Result<()>) -> String {
    let logs_dir = format!("{workspace_dir}/test-logs");

    if !exists(Path::new(&logs_dir)) {
        create(Path::new(&logs_dir)).unwrap();
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
    assert_initialized_with(&INITIALIZER);
}

fn assert_initialized_with(initializer: &Once) {
    assert!(
        initializer.is_completed(),
        "testing_aids tracing was used before initialize() ran; every test binary that \
         explicitly tests tracing or belongs to a package with a direct normal/dev tracing \
         dependency must install the subscriber from a `#[ctor::ctor]` \
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
    start_buffer_scope(is_mutation_testing)
}

fn start_buffer_scope(mutation_testing: impl FnOnce() -> bool) -> BufferGuard {
    // #[gamma::skip(call_result.default, reason = "assert_initialized returns unit; default_after still executes the assertion.")]
    assert_initialized();

    // Unlike `write_to_stdout`/`write_to_stdout_and_file`, buffer capture is not
    // disabled under mutation testing: its contents are asserted upon by the calling
    // test, so a disabled buffer would break those tests rather than merely omit
    // diagnostic output. The stdout tee, however, is purely diagnostic, so we silence
    // it under mutation testing to avoid the output overhead.
    // #[gamma::skip(call_result.default, reason = "enable_stdout returns unit; default_after still applies the mode to the flag.")]
    enable_stdout(&STDOUT_ENABLED, mutation_testing());

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
                    // #[gamma::skip(option.some_to_none, reason = "After successful file creation, unwrapping Some(Ok(())) is a no-op; None skips only that no-op.")]
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
struct LogWriter<W = File> {
    file: Option<W>,
}

impl<W: Write> Write for LogWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.file.as_mut().map_or(Ok(buf.len()), |file| file.write(buf))
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.file.as_mut().map_or(Ok(()), W::flush)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    #[serial_test::serial]
    fn nov4_public_stdout_switch_obeys_both_injected_modes_even_in_a_mutation_runner() {
        for mode in [false, true] {
            crate::test_hooks::with_mode(mode, || {
                let original = super::STDOUT_ENABLED.swap(false, super::Ordering::Relaxed);
                let outcome = std::panic::catch_unwind(super::write_to_stdout);
                let enabled = super::STDOUT_ENABLED.swap(original, super::Ordering::Relaxed);
                outcome.unwrap();
                assert_eq!(enabled, !mode);
            });
        }
    }

    #[test]
    fn nov4_log_writer_flush_forwards_the_in_memory_failure_instead_of_reporting_success() {
        struct Fails(std::rc::Rc<std::cell::Cell<usize>>);
        impl std::io::Write for Fails {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                self.0.set(self.0.get() + 1);
                Err(std::io::Error::from(std::io::ErrorKind::BrokenPipe))
            }
        }
        let calls = std::rc::Rc::new(std::cell::Cell::new(0));
        let mut writer = super::LogWriter {
            file: Some(Fails(std::rc::Rc::clone(&calls))),
        };
        assert_eq!(
            std::io::Write::flush(&mut writer).unwrap_err().kind(),
            std::io::ErrorKind::BrokenPipe
        );
        assert_eq!(calls.get(), 1);
        writer.file = None;
        std::io::Write::flush(&mut writer).unwrap();
        assert_eq!(calls.get(), 1);
    }
    #[test]
    #[serial_test::serial]
    fn nov4_public_stdout_switch_matches_the_current_mutation_mode_without_writing() {
        let original = super::STDOUT_ENABLED.swap(false, super::Ordering::Relaxed);
        let outcome = std::panic::catch_unwind(super::write_to_stdout);
        let enabled = super::STDOUT_ENABLED.swap(original, super::Ordering::Relaxed);
        outcome.unwrap();
        assert_eq!(enabled, !crate::is_mutation_testing());
    }

    #[test]
    #[serial_test::serial]
    fn nov4_mutation_stdout_mode_does_not_require_initialization_or_enable_output() {
        let initializer = super::Once::new();
        let original = super::STDOUT_ENABLED.swap(false, super::Ordering::Relaxed);
        let outcome = std::panic::catch_unwind(|| super::write_to_stdout_initialized(true, &initializer));
        let enabled = super::STDOUT_ENABLED.swap(original, super::Ordering::Relaxed);
        outcome.unwrap();
        assert!(!enabled);
        assert!(!initializer.is_completed());
    }

    #[test]
    #[serial_test::serial]
    fn gamma_round2_uninitialized_stdout_and_file_modes_reject_before_publishing_or_writing() {
        let initializer = super::Once::new();
        let before = super::STDOUT_ENABLED.load(super::Ordering::Relaxed);
        let stdout = std::panic::catch_unwind(|| super::write_to_stdout_initialized(false, &initializer)).unwrap_err();
        let file = std::panic::catch_unwind(|| {
            super::write_to_stdout_and_file_initialized("gamma_round2_must_not_be_written", false, &initializer)
        });
        let Err(file) = file else {
            panic!("uninitialized file output must be rejected before creating a guard");
        };
        for panic in [stdout, file] {
            let message = panic
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| panic.downcast_ref::<&str>().copied())
                .unwrap();
            assert!(message.starts_with("testing_aids tracing was used before initialize() ran"));
        }
        assert_eq!(super::STDOUT_ENABLED.load(super::Ordering::Relaxed), before);
    }

    use std::cell::RefCell;

    use super::*;

    #[test]
    #[serial_test::serial]
    fn gamma_round2_workspace_search_checks_each_ancestor_for_the_exact_marker() {
        let root = std::path::PathBuf::from("workspace");
        let member = root.join("member");
        let nested = member.join("nested");
        let visited = RefCell::new(Vec::new());
        let found = find_workspace_directory(nested.clone(), |path| {
            visited.borrow_mut().push(path.to_path_buf());
            path == root.join("Cargo.lock")
        });
        assert_eq!(found, root.to_string_lossy());
        assert_eq!(
            visited.into_inner(),
            vec![nested.join("Cargo.lock"), member.join("Cargo.lock"), root.join("Cargo.lock")]
        );
    }

    #[test]
    #[serial_test::serial]
    #[should_panic(expected = "No Cargo workspace found in ancestors of the working directory")]
    fn gamma_round2_workspace_search_reports_exhausted_ancestors() {
        find_workspace_directory(std::path::PathBuf::from("workspace"), |_| false);
    }

    #[test]
    #[serial_test::serial]
    fn gamma_round2_log_directory_creation_is_conditional_and_receives_the_exact_path() {
        for present in [false, true] {
            let created = RefCell::new(Vec::new());
            let path = ensure_logs_directory(
                "workspace",
                |path| {
                    assert_eq!(path, std::path::Path::new("workspace").join("test-logs"));
                    present
                },
                |path| {
                    created.borrow_mut().push(path.to_path_buf());
                    Ok(())
                },
            );
            assert_eq!(std::path::Path::new(&path), std::path::Path::new("workspace").join("test-logs"));
            assert_eq!(
                created.into_inner(),
                if present {
                    Vec::new()
                } else {
                    vec![std::path::PathBuf::from("workspace").join("test-logs")]
                }
            );
        }
    }

    #[test]
    #[serial_test::serial]
    #[should_panic(expected = "owned creation failure")]
    fn gamma_round2_log_directory_creation_propagates_the_injected_error() {
        ensure_logs_directory("workspace", |_| false, |_| Err(io::Error::other("owned creation failure")));
    }

    #[test]
    #[serial_test::serial]
    fn gamma_round2_stdout_decision_enables_only_nonmutation_diagnostics() {
        let previous = STDOUT_ENABLED.swap(false, Ordering::AcqRel);
        write_to_stdout_mode(true);
        let mutation = STDOUT_ENABLED.load(Ordering::Acquire);
        write_to_stdout_mode(false);
        let normal = STDOUT_ENABLED.load(Ordering::Acquire);
        STDOUT_ENABLED.store(previous, Ordering::Release);
        assert!(!mutation);
        assert!(normal);
    }

    #[test]
    #[serial_test::serial]
    #[cfg_attr(
        miri,
        ignore = "native tests cover workspace discovery; controlled directory fixtures retain Miri coverage"
    )]
    fn gamma_round2_workspace_and_log_paths_use_the_actual_workspace_root() {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let root = manifest.parent().unwrap().parent().unwrap();
        assert_eq!(std::path::Path::new(&workspace_directory()), root);
        assert_eq!(std::path::Path::new(&logs_directory()), root.join("test-logs"));
        assert_eq!(
            std::path::Path::new(&log_file("round2.log")),
            root.join("test-logs").join("round2.log")
        );
    }

    #[test]
    #[serial_test::serial]
    fn gamma_round2_buffer_guard_drop_and_consumption_clear_the_published_flag() {
        assert!(!buffer_active());
        let previous = STDOUT_ENABLED.swap(false, Ordering::AcqRel);
        let guard = start_buffer_scope(|| false);
        let normal_stdout = STDOUT_ENABLED.load(Ordering::Acquire);
        assert!(buffer_active());
        assert!(LOG_BUFFER.lock().unwrap().is_some());
        drop(guard);
        assert!(LOG_BUFFER.lock().unwrap().is_none());
        assert!(!buffer_active());
        STDOUT_ENABLED.store(false, Ordering::Release);
        let guard = start_buffer_scope(|| true);
        let mutation_stdout = STDOUT_ENABLED.load(Ordering::Acquire);
        assert!(buffer_active());
        assert!(guard.into_inner().is_empty());
        assert!(LOG_BUFFER.lock().unwrap().is_none());
        assert!(!buffer_active());
        STDOUT_ENABLED.store(previous, Ordering::Release);
        assert!(normal_stdout);
        assert!(!mutation_stdout);
    }

    #[test]
    #[serial_test::serial]
    fn gamma_round2_consumed_guard_drop_does_not_detach_a_replacement_buffer() {
        let guard = write_to_stdout_and_buffer();
        // Model a consumed guard finishing Drop after a replacement scope has attached.
        drop(BufferGuard { buffer: None });
        assert!(buffer_active());
        assert!(Arc::ptr_eq(
            LOG_BUFFER.lock().unwrap().as_ref().unwrap(),
            guard.buffer.as_ref().unwrap()
        ));
        drop(guard);
        assert!(!buffer_active());
    }

    #[test]
    #[serial_test::serial]
    #[cfg_attr(miri, ignore = "native tests cover file scopes; in-memory buffer ownership retains Miri coverage")]
    fn gamma_round2_file_scope_preserves_active_output_and_detaches_all_published_state() {
        assert!(!file_active());
        let previous_stdout = STDOUT_ENABLED.swap(false, Ordering::AcqRel);
        let filename = format!("gamma_round2_owned_output_{}.log", std::process::id());
        let path = log_file(&filename);
        let guard = write_to_stdout_and_file_mode(&filename, false);
        let normal_stdout = STDOUT_ENABLED.load(Ordering::Acquire);
        assert!(file_active());
        LOG_FILE.lock().unwrap().as_mut().unwrap().write_all(b"retained").unwrap();
        let duplicate = std::panic::catch_unwind(|| start_log_file_scope(&filename));
        let rejected = duplicate.is_err();
        if let Ok(duplicate) = duplicate {
            std::mem::forget(duplicate);
        }
        let bytes = fs::read(&path).unwrap();
        drop(guard);
        let detached = LOG_FILE.lock().unwrap().take();
        let was_detached = detached.is_none();
        drop(detached);
        let active = file_active();
        let enabled = FILE_ENABLED.swap(false, Ordering::AcqRel);
        fs::remove_file(path).unwrap();
        STDOUT_ENABLED.store(false, Ordering::Release);
        let suppressed = write_to_stdout_and_file_mode(&filename, true);
        let mutation_stdout = STDOUT_ENABLED.load(Ordering::Acquire);
        let fake = suppressed.fake;
        drop(suppressed);
        STDOUT_ENABLED.store(previous_stdout, Ordering::Release);
        assert!(rejected);
        assert_eq!(bytes, b"retained");
        assert!(was_detached);
        assert!(!active);
        assert!(!enabled);
        assert!(normal_stdout);
        assert!(!mutation_stdout);
        assert!(fake);
        assert!(LOG_FILE.lock().unwrap().is_none());
    }

    #[test]
    #[serial_test::serial]
    #[should_panic(expected = "called `Result::unwrap()` on an `Err` value")]
    #[cfg_attr(miri, ignore = "native tests cover file creation errors; buffer output retains Miri coverage")]
    fn gamma_round2_file_creation_error_is_reported_before_returning_a_guard() {
        let missing = std::path::PathBuf::from(format!("gamma_round2_missing_parent_{}", std::process::id())).join("output.log");
        let missing = missing.to_str().unwrap();
        assert!(!std::path::Path::new(&log_file(missing)).parent().unwrap().exists());
        let _guard = start_log_file_scope(missing);
    }

    #[test]
    #[serial_test::serial]
    fn gamma_survivor_buffer_commit_consumes_final_newline_and_partial_tail_exactly_once() {
        let (mut writer, buffer) = new_writer();
        writer.write_all(b"first\n\npartial").unwrap();
        writer.flush().unwrap();
        assert_eq!(*buffer.lock().unwrap(), ["first", ""]);
        assert_eq!(writer.pending, b"partial");
        writer.commit_final();
        writer.commit_final();
        assert_eq!(*buffer.lock().unwrap(), ["first", "", "partial"]);
        assert_eq!(writer.pending, Vec::<u8>::new());
    }

    #[test]
    #[serial_test::serial]
    fn gamma_survivor_absent_buffer_discards_bytes_and_reports_full_write_length() {
        let mut writer = BufferWriter {
            buffer: None,
            pending: b"stale".to_vec(),
        };
        assert_eq!(writer.write(b"new").unwrap(), 3);
        assert_eq!(writer.pending, b"stale");
        writer.flush().unwrap();
        assert_eq!(writer.pending, Vec::<u8>::new());
        let mut writer: LogWriter = LogWriter { file: None };
        for bytes in [b"".as_slice(), b"one".as_slice()] {
            assert_eq!(writer.write(bytes).unwrap(), bytes.len());
        }
        writer.flush().unwrap();
    }

    #[test]
    #[serial_test::serial]
    fn gamma_survivor_fake_file_guard_drop_never_requires_an_active_file() {
        assert!(LOG_FILE.lock().unwrap().is_none());
        let fake = FileGuard::fake();
        assert!(fake.fake);
        std::panic::catch_unwind(|| drop(fake)).unwrap();
        assert!(LOG_FILE.lock().unwrap().is_none());
        assert!(!FILE_ENABLED.load(Ordering::Acquire));
    }

    #[test]
    #[serial_test::serial]
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
    #[serial_test::serial]
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
    #[serial_test::serial]
    fn write_splits_on_newlines_into_separate_entries() {
        let (mut writer, buffer) = new_writer();

        writer.write_all(b"first line\nsecond line\n").unwrap();
        writer.flush().unwrap();

        assert_eq!(*buffer.lock().unwrap(), vec!["first line", "second line"]);
    }

    #[test]
    #[serial_test::serial]
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
    #[serial_test::serial]
    fn flush_retains_a_trailing_partial_line_until_terminated() {
        let (mut writer, buffer) = new_writer();

        writer.write_all(b"complete\npartial").unwrap();
        writer.flush().unwrap();

        // Only the newline-terminated line is committed; the partial line waits.
        assert_eq!(*buffer.lock().unwrap(), vec!["complete"]);
    }

    #[test]
    #[serial_test::serial]
    fn drop_flushes_a_final_line_without_a_trailing_newline() {
        let (mut writer, buffer) = new_writer();

        writer.write_all(b"no trailing newline").unwrap();
        drop(writer);

        assert_eq!(*buffer.lock().unwrap(), vec!["no trailing newline"]);
    }
}
