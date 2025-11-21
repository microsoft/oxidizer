// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::borrow::Cow;
use std::fmt;

/// Source location information (file and line).
#[derive(Debug, Clone)]
pub struct Location {
    /// File where the context was added
    pub file: &'static str,
    /// Line number where the context was added
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

/// A span entry that can include location information.
#[derive(Debug, Clone)]
pub struct SpanInfo {
    /// The span message
    pub message: Cow<'static, str>,
    /// Optional location where the span was added
    pub location: Option<Location>,
}

impl SpanInfo {
    /// Creates a new context with just a message.
    pub fn new(message: impl Into<Cow<'static, str>>) -> Self {
        Self {
            message: message.into(),
            location: None,
        }
    }

    /// Creates a new context with message, file, and line information.
    pub fn detailed(message: impl Into<Cow<'static, str>>, file: &'static str, line: u32) -> Self {
        Self {
            message: message.into(),
            location: Some(Location::new(file, line)),
        }
    }

    /// Returns true if this context has location information.
    #[must_use]
    pub const fn has_location(&self) -> bool {
        self.location.is_some()
    }
}

impl fmt::Display for SpanInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)?;
        if let Some(location) = &self.location {
            write!(f, " (at {location})")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trace_info() {
        let ctx1 = SpanInfo::new("simple span");
        assert!(!ctx1.has_location());
        assert_eq!(ctx1.to_string(), "simple span");

        let ctx2 = SpanInfo::detailed("detailed span", "main.rs", 42);
        assert!(ctx2.has_location());
        assert_eq!(ctx2.to_string(), "detailed span (at main.rs:42)");
    }
}
