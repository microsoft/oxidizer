// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#[derive(Debug)]
pub(crate) enum Error {
    DecodeAllocator(seismograph_rallocator::Error),
    UnsupportedSchema(u16),
    UnsupportedRuntimeSchema(u16),
    DuplicateAllocatorSource,
    DuplicateMimallocSource,
    SourceBackendMismatch { source: &'static str, backend: &'static str },
}

impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DecodeAllocator(error) => write!(formatter, "invalid allocator snapshot: {error}"),
            Self::UnsupportedSchema(schema) => write!(formatter, "unsupported allocator source schema {schema}"),
            Self::UnsupportedRuntimeSchema(schema) => write!(formatter, "unsupported runtime source schema {schema}"),
            Self::DuplicateAllocatorSource => write!(formatter, "multiple native allocator sources make the inventory ambiguous"),
            Self::DuplicateMimallocSource => write!(formatter, "multiple mimalloc allocator sources make the inventory ambiguous"),
            Self::SourceBackendMismatch { source, backend } => {
                write!(formatter, "allocator source {source} contains a {backend} payload")
            }
        }
    }
}

impl std::error::Error for Error {}
