// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Snapshot container framing.

use crate::snapshot::Version;

const WIRE_FORMAT_VERSION: u16 = 1;
const HEADER_LEN: usize = 20;
const SECTION_HEADER_LEN: usize = 8;

/// Header of a snapshot wire container.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Header {
    wire_format: u16,
    telemetry_schema: u16,
    producer: Version,
}

impl Header {
    /// Creates a header for the current wire format.
    #[must_use]
    pub(crate) const fn new(telemetry_schema: u16, producer: Version) -> Self {
        Self {
            wire_format: WIRE_FORMAT_VERSION,
            telemetry_schema,
            producer,
        }
    }

    /// Returns the encoded header length.
    #[must_use]
    pub(crate) const fn encoded_len() -> usize {
        HEADER_LEN
    }

    /// Returns the wire-format version.
    #[must_use]
    pub(crate) const fn wire_format(&self) -> u16 {
        self.wire_format
    }

    /// Returns the telemetry-schema version.
    #[must_use]
    pub(crate) const fn telemetry_schema(&self) -> u16 {
        self.telemetry_schema
    }

    /// Returns the producer version.
    #[must_use]
    pub(crate) const fn producer(&self) -> Version {
        self.producer
    }

    pub(crate) const fn from_wire(wire_format: u16, telemetry_schema: u16, producer: Version) -> Self {
        Self {
            wire_format,
            telemetry_schema,
            producer,
        }
    }
}

/// A decoded section of a snapshot wire container.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Section<'a> {
    id: u16,
    version: u16,
    payload: &'a [u8],
}

impl<'a> Section<'a> {
    /// Returns the encoded section-header length plus `payload_len`.
    #[must_use]
    pub(crate) const fn encoded_len(payload_len: usize) -> usize {
        SECTION_HEADER_LEN + payload_len
    }

    /// Returns the section identifier.
    #[must_use]
    pub(crate) const fn id(&self) -> u16 {
        self.id
    }

    /// Returns the section format version.
    #[must_use]
    pub(crate) const fn version(&self) -> u16 {
        self.version
    }

    /// Returns the section payload.
    #[must_use]
    pub(crate) const fn payload(&self) -> &'a [u8] {
        self.payload
    }

    pub(crate) const fn new(id: u16, version: u16, payload: &'a [u8]) -> Self {
        Self { id, version, payload }
    }
}

pub(crate) const fn wire_format_version() -> u16 {
    WIRE_FORMAT_VERSION
}

pub(crate) const fn magic() -> [u8; 8] {
    *b"RALSNAP\0"
}
