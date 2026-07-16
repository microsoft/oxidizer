// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// A dimension key for telemetry events.
///
/// Keys are always compile-time `'static` strings, so a `Key` is `Copy` and
/// never allocates — processing and snapshotting can retain it freely.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Key(&'static str);

impl Key {
    /// Returns the key as a string slice.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        self.0
    }
}

impl From<&'static str> for Key {
    fn from(value: &'static str) -> Self {
        Self(value)
    }
}

impl std::fmt::Display for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_from_static_str() {
        let key = Key::from("http.method");
        assert_eq!(key.as_str(), "http.method");
    }

    #[test]
    fn key_display_writes_label() {
        let key = Key::from("http.method");
        assert_eq!(key.to_string(), "http.method");
    }
}
