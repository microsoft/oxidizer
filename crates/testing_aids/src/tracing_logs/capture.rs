// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Log capture buffer for testing.

use std::io::Write;
use std::sync::Mutex;

use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::SubscriberExt;

/// Log capture buffer for testing.
///
/// Captures formatted log output into a shared buffer that can be inspected in
/// tests. Pair with
/// [`set_default`](tracing::subscriber::set_default) to scope capture to the current thread.
#[derive(Debug, Clone, Default)]
pub struct Capture {
    buffer: std::sync::Arc<Mutex<Vec<u8>>>,
}

impl Capture {
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
    /// Panics if the buffer cannot be locked for reading.
    #[must_use]
    pub fn output(&self) -> String {
        String::from_utf8_lossy(&self.buffer.lock().unwrap()).to_string()
    }

    /// Asserts that the captured log output contains the given string.
    ///
    /// # Panics
    ///
    /// Panics if the captured log output does not contain the given string.
    pub fn assert_contains(&self, expected: &str) {
        let output = self.output();
        assert!(
            output.contains(expected),
            "log output does not contain '{expected}', got:\n{output}"
        );
    }

    /// Creates a `tracing_subscriber` that writes to this capture buffer.
    ///
    /// Use with [`set_default`](tracing::subscriber::set_default) to scope capture to the
    /// current thread so parallel tests don't interfere with each other.
    ///
    /// # Panics
    ///
    /// Panics if the silent always-interested subscriber has not been installed by a
    /// `#[ctor::ctor]` process-initialization function calling
    /// [`initialize`](super::initialize). Thread-local capture only
    /// composes deterministically when that subscriber is present from process start.
    /// See `docs/tracing-tests.md`.
    #[must_use]
    pub fn subscriber(&self) -> impl tracing::Subscriber {
        super::output::assert_initialized();
        tracing_subscriber::registry().with(
            tracing_subscriber::fmt::layer()
                .without_time()
                .with_writer(CaptureWriterMaker {
                    buffer: std::sync::Arc::clone(&self.buffer),
                })
                .with_ansi(false),
        )
    }
}

/// Builds [`CaptureWriter`] instances for the `tracing_subscriber` formatting layer.
struct CaptureWriterMaker {
    buffer: std::sync::Arc<Mutex<Vec<u8>>>,
}

impl<'a> MakeWriter<'a> for CaptureWriterMaker {
    type Writer = CaptureWriter;

    fn make_writer(&'a self) -> Self::Writer {
        CaptureWriter {
            buffer: std::sync::Arc::clone(&self.buffer),
        }
    }
}

/// Writer that appends to a shared buffer.
#[derive(Debug)]
struct CaptureWriter {
    buffer: std::sync::Arc<Mutex<Vec<u8>>>,
}

impl Write for CaptureWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.buffer.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
