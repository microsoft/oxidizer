// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Source location metadata.

/// Location in source code where an event was emitted.
///
/// Captured automatically by the `emit!` macro at the call site.
#[derive(Debug, Clone, Copy)]
pub struct SourceLocation {
    crate_name: &'static str,
    file: &'static str,
    line: u32,
}

impl SourceLocation {
    /// Creates a new source location.
    #[must_use]
    pub const fn new(crate_name: &'static str, file: &'static str, line: u32) -> Self {
        Self { crate_name, file, line }
    }

    /// The crate name where the event was emitted.
    #[must_use]
    pub const fn crate_name(&self) -> &'static str {
        self.crate_name
    }

    /// The file path where the event was emitted.
    #[must_use]
    pub const fn file(&self) -> &'static str {
        self.file
    }

    /// The line number where the event was emitted.
    #[must_use]
    pub const fn line(&self) -> u32 {
        self.line
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_location_accessors_roundtrip() {
        let loc = SourceLocation::new("my_crate", "src/lib.rs", 42);
        assert_eq!(loc.crate_name(), "my_crate");
        assert_eq!(loc.file(), "src/lib.rs");
        assert_eq!(loc.line(), 42);
    }
}
