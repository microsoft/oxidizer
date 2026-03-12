// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Thread-local log capture buffer for testing.

use std::io::Write;
use std::sync::Mutex;

use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::SubscriberExt;

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
    #[must_use]
    pub fn output(&self) -> String {
        String::from_utf8_lossy(&self.buffer.lock().unwrap()).to_string()
    }

    /// Asserts that the captured log output contains the given string.
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
