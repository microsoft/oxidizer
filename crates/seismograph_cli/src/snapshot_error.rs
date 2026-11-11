// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::io;
use std::path::PathBuf;

#[derive(Debug)]
pub(crate) enum Error {
    Io(io::Error),
    DecodeContainer {
        seismograph: seismograph::Error,
        native: seismograph_rallocator::Error,
    },
    DecodeAllocator(seismograph_rallocator::Error),
    DecodeRuntime(seismograph_runtime::snapshot::Error),
    UnsupportedSchema(u16),
    UnsupportedRuntimeSchema(u16),
    DuplicateAllocatorSource,
    DuplicateMimallocSource,
    SourceBackendMismatch {
        source: &'static str,
        backend: &'static str,
    },
    SamePath(PathBuf),
    OutputExists(PathBuf),
}

impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "{error}"),
            Self::DecodeContainer { seismograph, native } => write!(
                formatter,
                "invalid snapshot: {seismograph}; native allocator snapshot decode also failed: {native}"
            ),
            Self::DecodeAllocator(error) => write!(formatter, "invalid allocator snapshot: {error}"),
            Self::DecodeRuntime(error) => write!(formatter, "invalid runtime snapshot: {error}"),
            Self::UnsupportedSchema(schema) => write!(formatter, "unsupported allocator source schema {schema}"),
            Self::UnsupportedRuntimeSchema(schema) => write!(formatter, "unsupported runtime source schema {schema}"),
            Self::DuplicateAllocatorSource => write!(formatter, "multiple native allocator sources make the inventory ambiguous"),
            Self::DuplicateMimallocSource => write!(formatter, "multiple mimalloc allocator sources make the inventory ambiguous"),
            Self::SourceBackendMismatch { source, backend } => {
                write!(formatter, "allocator source {source} contains a {backend} payload")
            }
            Self::SamePath(path) => write!(formatter, "input and output refer to the same path: {}", path.display()),
            Self::OutputExists(path) => write!(formatter, "refusing to overwrite existing output: {}", path.display()),
        }
    }
}

impl std::error::Error for Error {}
