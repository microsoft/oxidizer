// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::borrow::Cow;
use std::fmt;

/// Source location information (file and line).
#[derive(Debug, Clone)]
pub struct Location {
    /// File where the trace was added
    pub file: &'static str,
    /// Line number where the trace was added
    pub line: u32,
}

impl Location {
    /// Creates a new location with file and line information.
    #[must_use]
    pub const fn new(file: &'static str, line: u32) -> Self {
        Self { file, line }
    }
}

impl fmt::Display for Location {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.file, self.line)
    }
}

/// A trace entry that can include location information.
#[derive(Debug, Clone)]
pub struct TraceInfo {
    /// The trace message
    pub message: Cow<'static, str>,
    /// Location where the trace was added
    pub location: Location,
}

impl TraceInfo {
    /// Creates a new trace with message, file, and line information.
    pub fn new(message: impl Into<Cow<'static, str>>, file: &'static str, line: u32) -> Self {
        Self {
            message: message.into(),
            location: Location::new(file, line),
        }
    }
}

impl fmt::Display for TraceInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} (at {})", self.message, self.location)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trace_info() {
        let ctx = TraceInfo::new("trace message", "main.rs", 42);
        assert_eq!(ctx.to_string(), "trace message (at main.rs:42)");
        assert_eq!(ctx.location.file, "main.rs");
        assert_eq!(ctx.location.line, 42);
    }
}
