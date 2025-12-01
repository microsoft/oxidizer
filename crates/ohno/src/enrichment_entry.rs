// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::borrow::Cow;
use std::fmt;

/// Source location information (file and line).
#[derive(Debug, Clone)]
pub struct Location {
    /// File where the enrichment was added
    pub file: &'static str,
    /// Line number where the enrichment was added
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

/// An enrichment entry containing a message and its source location.
#[derive(Debug, Clone)]
pub struct EnrichmentEntry {
    /// The enrichment message
    pub message: Cow<'static, str>,
    /// Location where the enrichment was added
    pub location: Location,
}

impl EnrichmentEntry {
    /// Creates a new enrichment entry with message, file, and line information.
    pub fn new(message: impl Into<Cow<'static, str>>, file: &'static str, line: u32) -> Self {
        Self {
            message: message.into(),
            location: Location::new(file, line),
        }
    }
}

impl fmt::Display for EnrichmentEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} (at {})", self.message, self.location)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_enrichment_entry() {
        let ctx = EnrichmentEntry::new("enrichment message", "main.rs", 42);
        assert_eq!(ctx.to_string(), "enrichment message (at main.rs:42)");
        assert_eq!(ctx.location.file, "main.rs");
        assert_eq!(ctx.location.line, 42);
    }
}
