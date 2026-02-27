// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fs::{self, File};
use std::io::Write;
use std::sync::{Mutex, Once};

use tracing::Level;
use tracing::level_filters::LevelFilter;
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
/// is compatible with `log_to_stdout_and_file()`, as well - you can log to both standard
/// output and file at the same time and upgrade from one to the other at will.
///
/// All log entries will be written synchronously to ensure no data gets lost in case of
/// test failure or other anomaly.
///
/// Logging is disabled under mutation testing - this becomes a no-op.
pub fn log_to_stdout() {
    if is_mutation_testing() {
        // Under mutation testing, we do not log anything, to speed up the tests.
        return;
    }

    ensure_logging_initialized();
}

/// Enables logging of test output to the standard output stream and to a specific log file.
///
/// Terminal output is limited to INFO and above, while the log file captures all messages.
///
/// The log file is saved in a "test-logs" directory under the Cargo workspace root.
///
/// Logging is global state and will last until end of process - once you call this, all logging
/// statements in the process will be captured and emitted to the standard output,
/// as well as logged to file. After the returned `LogFileGuard` is dropped, future log entries
/// will only go to the standard output.
///
/// All log entries will be written synchronously to ensure no data gets lost in case of
/// test failure or other anomaly.
///
/// Logging is disabled under mutation testing - this becomes a no-op.
pub fn log_to_stdout_and_file(file_name: &str) -> LogFileGuard {
    if is_mutation_testing() {
        // Under mutation testing, we do not log anything, to speed up the tests.
        return LogFileGuard::fake();
    }

    ensure_logging_initialized();

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

fn ensure_logging_initialized() {
    LOGGING_INITIALIZER.call_once(|| {
        let terminal_layer = tracing_subscriber::fmt::layer().with_filter(LevelFilter::from_level(Level::INFO));

        let file_layer = tracing_subscriber::fmt::layer()
            .with_writer(LogFileWriter)
            // No coloring codes or such fancy stuff in the file, please.
            .with_ansi(false);

        tracing_subscriber::registry()
            .with(terminal_layer)
            .with(file_layer)
            .try_init()
            .expect("this can only happen if something else besides testing_aids has configured logging");
    });
}

static LOGGING_INITIALIZER: Once = Once::new();

/// We write log entries to this file (if not `None`).
static LOG_FILE: Mutex<Option<File>> = Mutex::new(None);

/// Defines the scope within which all log entries (from all threads) go to a log file.
///
/// Once this scope ends, new log entries will no longer go to a file.
#[derive(Debug)]
#[must_use]
pub struct LogFileGuard {
    // Under mutation testing, we pretend to log but do not actually do so, to speed up the tests.
    fake: bool,
}

impl LogFileGuard {
    const fn new() -> Self {
        Self { fake: false }
    }

    const fn fake() -> Self {
        Self { fake: true }
    }
}

impl Drop for LogFileGuard {
    fn drop(&mut self) {
        if self.fake {
            return;
        }

        let mut log_file = LOG_FILE.lock().unwrap();

        assert!(log_file.is_some());
        *log_file = None;
    }
}

fn start_log_file_scope(file_name: &str) -> LogFileGuard {
    let file = File::create(log_file(file_name)).unwrap();

    {
        let mut log_file = LOG_FILE.lock().unwrap();

        assert!(
            log_file.is_none(),
            "a log file is already active; multiple tests cannot log to file in parallel within the same process - logging is global state"
        );

        *log_file = Some(file);
    }

    LogFileGuard::new()
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

/// Thread-local log capture buffer for testing.
///
/// Uses `tracing_subscriber::fmt::MakeWriter` to capture formatted log output
/// into a thread-local buffer that can be inspected in tests.
#[derive(Debug, Clone, Default)]
pub struct LogCapture {
    buffer: std::sync::Arc<Mutex<Vec<u8>>>,
}

impl LogCapture {
    #[must_use]
    pub fn new() -> Self {
        Self {
            buffer: std::sync::Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Returns the captured log output as a string.
    ///
    /// # Panics
    ///
    /// Panics if the captured bytes are not valid UTF-8, which should not happen since `tracing_subscriber` writes UTF-8.
    #[must_use]
    pub fn output(&self) -> String {
        String::from_utf8_lossy(&self.buffer.lock().unwrap()).to_string()
    }

    /// Asserts that the captured log output contains the given string.
    ///
    /// # Panics
    ///
    /// Panics if the captured log output does not contain the expected string.
    pub fn assert_contains(&self, expected: &str) {
        let output = self.output();
        assert!(
            output.contains(expected),
            "log output does not contain '{expected}', got:\n{output}"
        );
    }

    /// Creates a `tracing_subscriber` that writes to this capture buffer.
    /// Use with `set_default()` for thread-local capture.
    #[must_use]
    pub fn subscriber(&self) -> impl tracing::Subscriber {
        tracing_subscriber::registry().with(tracing_subscriber::fmt::layer().with_writer(self.clone()).with_ansi(false))
    }
}

impl<'a> MakeWriter<'a> for LogCapture {
    type Writer = LogCaptureWriter;

    fn make_writer(&'a self) -> Self::Writer {
        LogCaptureWriter {
            buffer: std::sync::Arc::clone(&self.buffer),
        }
    }
}

/// Writer that appends to a shared buffer.
#[derive(Debug)]
pub struct LogCaptureWriter {
    buffer: std::sync::Arc<Mutex<Vec<u8>>>,
}

impl Write for LogCaptureWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.buffer.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
