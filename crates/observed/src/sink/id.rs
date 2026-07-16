// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Sink identity id.

use std::any::type_name;

/// A unique identity token for a sink.
///
/// Two ids are equal when they share the same label.
///
/// `SinkId` is `Copy` - pass it by value, not by reference.
#[derive(Copy, Clone, thread_aware::ThreadAware)]
pub struct SinkId {
    label: &'static str,
}

impl SinkId {
    /// Creates a new id with a human-readable label (used in debug output only).
    #[must_use]
    pub const fn new(label: &'static str) -> Self {
        Self { label }
    }

    /// Returns the human-readable label.
    #[must_use]
    pub const fn label(&self) -> &'static str {
        self.label
    }
}

impl PartialEq for SinkId {
    fn eq(&self, other: &Self) -> bool {
        self.label == other.label
    }
}

impl Eq for SinkId {}

impl std::hash::Hash for SinkId {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.label.hash(state);
    }
}

impl std::fmt::Debug for SinkId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct(type_name::<Self>()).field("label", &self.label).finish()
    }
}

impl From<&'static str> for SinkId {
    fn from(label: &'static str) -> Self {
        Self::new(label)
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use std::hash::{Hash, Hasher};

    use super::*;

    fn hash_of(id: &SinkId) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        id.hash(&mut hasher);
        hasher.finish()
    }

    #[test]
    fn hash_depends_on_label() {
        assert_eq!(hash_of(&SinkId::new("a")), hash_of(&SinkId::new("a")));
        assert_ne!(hash_of(&SinkId::new("a")), hash_of(&SinkId::new("b")));
    }

    #[test]
    fn from_static_str_equals_new() {
        // An id built from a string literal must be equal to one built via
        // `SinkId::new` with the same label, so a service constructed
        // with `builder("app")` is still targetable by a static
        // `SinkId::new("app")` used in `enrich_for(ID, …)`.
        static APP: SinkId = SinkId::new("app");
        let from_str: SinkId = "app".into();
        assert_eq!(from_str, APP);
        assert_eq!(from_str.label(), "app");
    }
}
