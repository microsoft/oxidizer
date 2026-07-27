// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// The resource whose configured deserialization limit was exceeded.
#[derive(Copy, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[non_exhaustive]
pub enum DeserializationResource {
    /// Nested values exceeded the configured depth.
    Depth,
    /// A sequence contained too many elements.
    SequenceLength,
    /// A map contained too many entries.
    MapLength,
    /// A decoded string contained too many bytes.
    StringLength,
    /// A decoded byte string contained too many bytes.
    ByteStringLength,
}

/// Details about a deserialization resource-limit violation.
#[derive(Copy, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct LimitExceeded {
    resource: DeserializationResource,
    limit: usize,
}

impl LimitExceeded {
    pub(super) const fn new(resource: DeserializationResource, limit: usize) -> Self {
        Self { resource, limit }
    }

    /// Returns the resource that exceeded its configured limit.
    #[must_use]
    pub const fn resource(self) -> DeserializationResource {
        self.resource
    }

    /// Returns the configured maximum value.
    #[must_use]
    pub const fn limit(self) -> usize {
        self.limit
    }
}
