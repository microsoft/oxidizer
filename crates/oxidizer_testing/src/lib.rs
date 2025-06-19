// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::marker::PhantomPinned;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;
use std::{fs, process, ptr, thread};

use rand::RngCore;
use tracing::Level;
use tracing::level_filters::LevelFilter;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::Layer;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

mod yielding;

pub use yielding::*;

/// If something (whatever) does not happen in a test within this time, the test will fail.
///
/// We are conservative here and allow much time - this is only to break out of infinite loops, not for any
/// situations that are actually expected.
///
/// This should be significantly smaller than the .cargo/mutants.toml timeout because multiple
/// tests may be executed during a single cargo-mutants run, so this timeout might not start
/// immediately at the start of a test run.
pub const TEST_TIMEOUT: Duration = Duration::from_secs(10);

/// Executes a thread-safe function on a background thread and abandons it if
/// it does not complete before the provided timeout.
///
/// # Panics
///
/// Panics if the test panics or the test timeout is exceeded.
#[cfg_attr(test, mutants::skip)] // This is test logic - pointless to mutate.
#[must_use]
pub fn execute_or_abandon<F, R>(f: F) -> Option<R>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    let (sender, receiver) = mpsc::channel();

    // There are multiple ways for the called function to fail:
    // 1. It fails to finish in the allowed time span.
    // 2. It panics, so the result is never sent.
    //
    // In both cases, the channel will get closed and recv_timeout
    // will signal an error saying the channel is broken.
    thread::spawn(move || {
        let result = f();
        sender.send(result).unwrap();
    });

    receiver.recv_timeout(TEST_TIMEOUT).ok()
}

/// Executes a function on the current thread and sets up a watchdog timer that terminates the
/// process if the target function does not complete before the provided timeout.
///
/// This is a variant of `execute_or_abandon()` that can be used with single-threaded
/// logic that does not support being moved to a background thread.
///
/// # Panics
///
/// Panics if the test panics or the test timeout is exceeded.
#[cfg_attr(test, mutants::skip)] // This is test logic - pointless to mutate.
pub fn execute_or_terminate_process<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    let (sender, receiver) = mpsc::channel();

    let watchdog = thread::Builder::new()
        .name("test watchdog".to_string())
        .spawn(move || if receiver.recv_timeout(TEST_TIMEOUT) == Ok(()) {} else {
            eprintln!("Test timed out, terminating process.");
            #[expect(
                clippy::exit,
                reason = "test harness is intentionally termianting test process that cannot continue execution"
            )]
            // Arbitrary value in portable range (8 bits) to signal "emergency timeout".
            process::exit(112);
        })
        .unwrap();

    let result = catch_unwind(AssertUnwindSafe(f));

    // We signal "done" no matter whether it panics or succeeds, all we care about is timeout.
    sender.send(()).unwrap();

    // We must wait for this to finish, otherwise Miri leak detection will be angry at us.
    watchdog.join().unwrap();

    // This will re-raise any panic if one occurred.
    result.unwrap()
}

/// Wires up an asynchronous console log output, intended for use in examples and tests. The
/// returned guard will ensure that all contents are flushed on drop, ensuring test isolation.
///
/// # Panics
///
/// Panics if logging has already been configured for this process. Logging can only be configured
/// once per process (because we set a global config to catch all logs from all threads). This
/// means you must use the nextest test runner to run tests using this capability, as nextest
/// isolates each test to its own process.
#[cfg_attr(test, mutants::skip)] // This is test logic - pointless to mutate.
pub fn log_to_console() -> WorkerGuard {
    let (non_blocking_stdout, guard) = tracing_appender::non_blocking(std::io::stdout());
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_writer(non_blocking_stdout)
        .try_init()
        .expect("logging already configured - you must use the nextest test runner to run this test, so each test is isolated into its own process; alternatively, run one test at a time");

    guard
}

/// Wires up an asynchronous console and file log output, intended for use in examples and tests.
///
/// The console logger will emit INFO and higher messages.
/// The file logger will emit all messages.
///
/// The log file is saved in a "logs" directory under the Cargo workspace root.
///
/// The returned guard array will ensure that all contents are flushed on drop, ensuring test
/// isolation. Just keep the array alive as long as anything may be producing logs.
///
/// # Panics
///
/// Panics if logging has already been configured for this process. Logging can only be configured
/// once per process (because we set a global config to catch all logs from all threads). This
/// means you must use the nextest test runner to run tests using this capability, as nextest
/// isolates each test to its own process.
#[cfg_attr(test, mutants::skip)] // This crate is only used during testing, no point testing our test code.
pub fn log_to_console_and_file(file_name: &str) -> WorkerGuard {
    let file_name = log_file(file_name);

    // Delete the file first, so we clean any old garbage from previous test run.
    _ = fs::remove_file(&file_name);

    let (non_blocking_stdout, guard) = tracing_appender::non_blocking(std::io::stdout());

    // We do not use a non-blocking layer for file, to ensure everything gets logged to file
    // even if a test times out or gets terminated for whatever reason.
    let file_appender = tracing_appender::rolling::never(".", &file_name);

    let console_layer = tracing_subscriber::fmt::layer()
        .with_writer(non_blocking_stdout)
        .with_filter(LevelFilter::from_level(Level::INFO));

    let file_layer = tracing_subscriber::fmt::layer()
        .with_writer(file_appender)
        // No coloring codes or such fancy stuff in the file, please.
        .with_ansi(false);

    tracing_subscriber::registry()
        .with(console_layer)
        .with(file_layer)
        .try_init()
        .expect("logging already configured - you must use the nextest test runner to run this test, so each test is isolated into its own process; alternatively, run one test at a time");

    guard
}

/// Looks upward in the filesystem from the current directory until it finds a directory with a
/// "Cargo.lock" file, indicating the root of a Cargo workspace. Returns the path to that directory.
#[cfg_attr(test, mutants::skip)] // This crate is only used during testing, no point testing our test code.
fn workspace_directory() -> String {
    let mut dir = std::env::current_dir().unwrap();

    loop {
        if dir.join("Cargo.lock").exists() {
            return dir.to_string_lossy().to_string();
        }

        assert!(
            dir.pop(),
            "No Cargo workspace found in ancestors of the working directory"
        );
    }
}

/// Returns the path to the directory where logs are written and ensures that this directory exists.
///
/// # Panics
///
/// Panics if the directory cannot be created or accessed.
#[cfg_attr(test, mutants::skip)] // This crate is only used during testing, no point testing our test code.
#[must_use]
pub fn logs_directory() -> String {
    let workspace_dir = workspace_directory();
    let logs_dir = format!("{workspace_dir}/test-logs");

    if !std::path::Path::new(&logs_dir).exists() {
        fs::create_dir_all(&logs_dir).unwrap();
    }

    logs_dir
}

/// Returns the path to a specified log file within the directory where logs are written and ensures that this directory exists.
#[cfg_attr(test, mutants::skip)] // This crate is only used during testing, no point testing our test code.
#[must_use]
pub fn log_file(file_name: &str) -> String {
    format!("{}/{}", logs_directory(), file_name)
}

/// Wraps a `T` and hands out static references to it, even though `T` is not actually static.
/// This is useful for mocking in tests, where we want to pretend that the mocks are 'static.
#[derive(Debug)]
pub struct FakeStatic<T: 'static> {
    value: T,
    _must_be_pinned: PhantomPinned,
}

impl<T: 'static> FakeStatic<T> {
    /// # Safety
    ///
    /// The caller must keep this instance alive as long as any returned 'static reference is alive.
    #[cfg_attr(test, mutants::skip)] // This is test logic - pointless to mutate.
    pub unsafe fn new(value: T) -> Pin<Box<Self>> {
        // A FakeStatic is always pinned to ensure the references we return via intermediate
        // pointers remain valid for the entire lifetime of the FakeStatic.
        Box::pin(Self {
            value,
            _must_be_pinned: PhantomPinned,
        })
    }

    #[cfg_attr(test, mutants::skip)] // This is test logic - pointless to mutate.
    pub const fn as_static(&self) -> &'static T {
        let ptr = ptr::from_ref(&self.value);

        // SAFETY: We forward the safety guarantee from the constructor.
        unsafe { &*ptr }
    }
}

/// Makes a noise whenever a clone is added/removed, to help understand when/where cloning occurs.
#[derive(Debug)]
pub struct CloneCanary {
    tag: u64,
    count: Arc<AtomicUsize>,
}

impl CloneCanary {
    #[cfg_attr(test, mutants::skip)] // This is test logic - pointless to mutate.
    #[must_use]
    pub fn new() -> Self {
        let tag = rand::rng().next_u64();

        eprintln!("CloneCanary{tag}: 0 -> 1");

        Self {
            count: Arc::new(AtomicUsize::new(1)),
            tag,
        }
    }
}

impl Clone for CloneCanary {
    #[cfg_attr(test, mutants::skip)] // This is test logic - pointless to mutate.
    fn clone(&self) -> Self {
        let prev_count = self.count.fetch_add(1, Ordering::Relaxed);

        let tag = self.tag;

        eprintln!(
            "CloneCanary{tag}: {prev_count} -> {}",
            prev_count.wrapping_add(1)
        );

        Self {
            count: Arc::clone(&self.count),
            tag: self.tag,
        }
    }
}

impl Drop for CloneCanary {
    #[cfg_attr(test, mutants::skip)] // This is test logic - pointless to mutate.
    fn drop(&mut self) {
        let prev_count = self.count.fetch_sub(1, Ordering::Relaxed);

        let tag = self.tag;

        eprintln!(
            "CloneCanary{tag}: {prev_count} -> {}",
            prev_count.wrapping_sub(1)
        );
    }
}

impl Default for CloneCanary {
    #[cfg_attr(test, mutants::skip)] // This is test logic - pointless to mutate.
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use super::*;

    #[mockall::automock]
    trait Whatever {
        fn do_something(&self, value: i32) -> i32;

        fn clone(&self) -> Self;
    }

    #[test]
    fn fake_static_mock_works() {
        let mut mock = MockWhatever::new();

        mock.expect_clone().times(1).returning(|| {
            let mut clone = MockWhatever::new();

            clone
                .expect_do_something()
                .times(1)
                .withf_st(|a| *a == 24)
                .returning_st(|a| a - 1);

            clone
        });

        mock.expect_do_something()
            .times(1)
            .withf_st(|a| *a == 42)
            .returning_st(|a| a + 1);

        // SAFETY: Wololo.
        let fake_static = unsafe { FakeStatic::new(mock) };

        let whatever = fake_static.as_static();
        assert_eq!(whatever.do_something(42), 43);

        let clone = whatever.clone();
        assert_eq!(clone.do_something(24), 23);
    }

    struct DropCheck {
        dropped: Rc<Cell<bool>>,
    }

    impl Drop for DropCheck {
        fn drop(&mut self) {
            self.dropped.set(true);
        }
    }

    #[test]
    fn fake_static_drops_inner() {
        let dropped = Rc::new(Cell::new(false));

        {
            let dc = DropCheck {
                dropped: Rc::clone(&dropped),
            };

            // SAFETY: Wololo.
            let _ = unsafe { FakeStatic::new(dc) };
        }

        assert!(dropped.get());
    }
}