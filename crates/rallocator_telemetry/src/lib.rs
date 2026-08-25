// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(
    clippy::map_err_ignore,
    reason = "Wire and integer conversion errors intentionally collapse into stable telemetry error categories"
)]
#![expect(
    clippy::missing_errors_doc,
    reason = "Public encoding and decoding functions return the crate's documented Error type"
)]
#![expect(
    clippy::module_name_repetitions,
    reason = "Schema field names remain explicit and stable when viewed independently"
)]
#![expect(
    clippy::struct_field_names,
    reason = "Wire schema fields retain explicit qualified names for standalone diagnostics and compatibility"
)]
#![expect(
    clippy::renamed_function_params,
    reason = "Implementation parameter names are clearer than generic trait names"
)]
#![expect(
    clippy::too_many_lines,
    reason = "Wire decoders remain linear so field order and validation are auditable against the schema"
)]

//! Owned rallocator snapshot schema and binary encoding.
//!
//! Snapshot data is organized into [`snapshot`], [`topology`], and
//! [`callers`]. The root exports encoding functions and their shared error.
//!
//! # Compatibility contract
//!
//! A snapshot has three layers with independent versions:
//!
//! - `rallocator_wire` owns the little-endian container header and
//!   length-prefixed section framing. A framing change increments the wire
//!   version, and readers reject unknown wire versions.
//! - This crate owns the telemetry schema named by the header. A change that
//!   reinterprets the snapshot as a whole increments that schema; readers
//!   reject unsupported schema versions.
//! - Each section owns its payload version. Compatible extensions increment
//!   only that section version. Unknown sections and unsupported optional
//!   section versions are skipped and reported through
//!   [`snapshot::Snapshot::skipped_sections`].
//!
//! Metadata and statistics sections are required. Historical section versions
//! accepted by the decoder receive documented neutral defaults for fields that
//! did not yet exist. Producers must not change the meaning or byte order of an
//! existing version.

pub mod callers;
pub mod snapshot;
pub mod topology;

use std::collections::{HashMap, HashSet};

use callers::{AddressLookup, Callers, Event, EventKind, HeapKind, ThreadLog, ThreadName};
use rallocator_wire::format::{Header, Section};
use rallocator_wire::io::{Reader, Writer};
use rallocator_wire::{Decode, Encode};
use snapshot::{Domain, Estimate, Histograms, Region, SizeClass, SkippedSectionFields, Snapshot, Stats};
use topology::{Segment, Slice, SliceKind, TopologyRegion};

const TELEMETRY_SCHEMA_VERSION: u16 = 1;

const SECTION_METADATA: u16 = 1;
const SECTION_STATS: u16 = 2;
const SECTION_SIZE_CLASSES: u16 = 3;
const SECTION_REGIONS: u16 = 4;
const SECTION_CALLERS: u16 = 5;
const SECTION_ADDRESSES: u16 = 6;
const SECTION_TOPOLOGY: u16 = 7;
const SECTION_DOMAINS: u16 = 8;
const SECTION_HISTOGRAMS: u16 = 9;
const SECTION_VERSION: u16 = 1;
const TOPOLOGY_SECTION_VERSION: u16 = 2;
const CALLERS_DIAGNOSTICS_VERSION: u16 = 2;
const CALLERS_THREAD_NAMES_VERSION: u16 = 3;
const CALLERS_STACK_TABLE_VERSION: u16 = 4;
const CALLERS_SECTION_VERSION: u16 = CALLERS_STACK_TABLE_VERSION;
const STATS_PAYLOAD_LEN: usize = 13 * 8;

/// An error reported while encoding or decoding a telemetry snapshot.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct Error {
    kind: ErrorKind,
}

/// Stable category of a telemetry encoding or decoding error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ErrorKind {
    /// The encoded snapshot length cannot be represented.
    LengthOverflow,
    /// The output buffer is too small.
    OutputTooSmall,
    /// The output buffer does not have the exact encoded length.
    OutputLengthMismatch,
    /// The wire container is invalid.
    Wire(rallocator_wire::Error),
    /// The telemetry schema version is unsupported.
    UnsupportedSchema(u16),
    /// A required section is missing.
    MissingSection(u16),
    /// A section appears more than once.
    DuplicateSection(u16),
    /// A section payload is malformed.
    MalformedSection(u16),
    /// A decoded integer cannot fit in the target type.
    IntegerOverflow,
    /// A section contains invalid UTF-8.
    InvalidUtf8(u16),
    /// An allocation event kind is unknown.
    UnknownEventKind(u8),
    /// A topology slice kind is unknown.
    UnknownSliceKind(u8),
}

impl Error {
    const LENGTH_OVERFLOW: Self = Self::new(ErrorKind::LengthOverflow);
    const OUTPUT_TOO_SMALL: Self = Self::new(ErrorKind::OutputTooSmall);
    const OUTPUT_LENGTH_MISMATCH: Self = Self::new(ErrorKind::OutputLengthMismatch);
    const INTEGER_OVERFLOW: Self = Self::new(ErrorKind::IntegerOverflow);

    const fn new(kind: ErrorKind) -> Self {
        Self { kind }
    }

    /// Returns the stable category of this error.
    #[must_use]
    pub const fn kind(self) -> ErrorKind {
        self.kind
    }

    const fn wire(error: rallocator_wire::Error) -> Self {
        Self::new(ErrorKind::Wire(error))
    }

    const fn unsupported_schema(version: u16) -> Self {
        Self::new(ErrorKind::UnsupportedSchema(version))
    }

    const fn missing_section(section: u16) -> Self {
        Self::new(ErrorKind::MissingSection(section))
    }

    const fn duplicate_section(section: u16) -> Self {
        Self::new(ErrorKind::DuplicateSection(section))
    }

    const fn malformed_section(section: u16) -> Self {
        Self::new(ErrorKind::MalformedSection(section))
    }

    const fn invalid_utf8(section: u16) -> Self {
        Self::new(ErrorKind::InvalidUtf8(section))
    }

    const fn unknown_event_kind(value: u8) -> Self {
        Self::new(ErrorKind::UnknownEventKind(value))
    }

    const fn unknown_slice_kind(value: u8) -> Self {
        Self::new(ErrorKind::UnknownSliceKind(value))
    }
}

impl From<rallocator_wire::Error> for Error {
    fn from(value: rallocator_wire::Error) -> Self {
        Self::wire(value)
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.kind {
            ErrorKind::LengthOverflow => formatter.write_str("the encoded snapshot length cannot be represented"),
            ErrorKind::OutputTooSmall => formatter.write_str("the snapshot output buffer is too small"),
            ErrorKind::OutputLengthMismatch => formatter.write_str("the snapshot output buffer must have the exact encoded length"),
            ErrorKind::Wire(error) => write!(formatter, "invalid wire container: {error}"),
            ErrorKind::UnsupportedSchema(version) => write!(formatter, "telemetry schema version {version} is unsupported"),
            ErrorKind::MissingSection(section) => write!(formatter, "required telemetry section {section} is missing"),
            ErrorKind::DuplicateSection(section) => write!(formatter, "telemetry section {section} appears more than once"),
            ErrorKind::MalformedSection(section) => write!(formatter, "telemetry section {section} is malformed"),
            ErrorKind::IntegerOverflow => formatter.write_str("a decoded integer cannot fit in the target type"),
            ErrorKind::InvalidUtf8(section) => write!(formatter, "telemetry section {section} contains invalid UTF-8"),
            ErrorKind::UnknownEventKind(value) => write!(formatter, "event kind {value} is unknown"),
            ErrorKind::UnknownSliceKind(value) => write!(formatter, "slice kind {value} is unknown"),
        }
    }
}

impl std::fmt::Debug for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "Error({self})")
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.kind {
            ErrorKind::Wire(error) => Some(error),
            _ => None,
        }
    }
}

/// Returns the exact byte length required to encode `snapshot`.
pub fn encoded_len(snapshot: &Snapshot) -> Result<usize, Error> {
    count(snapshot.size_classes.len())?;
    count(snapshot.regions.len())?;
    let size_classes = checked_add(4, checked_mul(snapshot.size_classes.len(), 4 + 8 + 9 * 8)?)?;
    let regions = checked_add(4, checked_mul(snapshot.regions.len(), 4 + 3 * 8)?)?;
    let topology = topology_encoded_len(&snapshot.topology)?;
    let domains = domains_encoded_len(&snapshot.domains)?;
    let callers = callers_encoded_len(snapshot.callers.as_ref())?;
    let histograms = histograms_encoded_len(&snapshot.histograms)?;
    let addresses = addresses_encoded_len(&snapshot.addresses)?;
    let payloads = [
        8,
        STATS_PAYLOAD_LEN,
        size_classes,
        regions,
        topology,
        domains,
        callers,
        histograms,
        addresses,
    ];
    payloads.into_iter().try_fold(Header::encoded_len(), |total, payload| {
        u32::try_from(payload).map_err(|_| Error::LENGTH_OVERFLOW)?;
        checked_add(total, checked_add(Section::encoded_len(0), payload)?)
    })
}

/// Encodes `snapshot` into an exactly sized output buffer.
pub fn encode(snapshot: &Snapshot, output: &mut [u8]) -> Result<usize, Error> {
    let expected = encoded_len(snapshot)?;
    if output.len() < expected {
        return Err(Error::OUTPUT_TOO_SMALL);
    }
    if output.len() != expected {
        return Err(Error::OUTPUT_LENGTH_MISMATCH);
    }

    let mut writer = Writer::new(output);
    let header = Header::new(TELEMETRY_SCHEMA_VERSION, snapshot.metadata.producer_version);
    writer.write_header(header)?;

    writer.begin_section(SECTION_METADATA, SECTION_VERSION, 8)?;
    writer.write_u64(snapshot.metadata.capture_duration_nanos)?;

    writer.begin_section(SECTION_STATS, SECTION_VERSION, STATS_PAYLOAD_LEN)?;
    write_stats(&mut writer, snapshot.stats)?;

    let size_classes_len = checked_add(4, checked_mul(snapshot.size_classes.len(), 4 + 8 + 9 * 8)?)?;
    writer.begin_section(SECTION_SIZE_CLASSES, SECTION_VERSION, size_classes_len)?;
    writer.write_u32(count(snapshot.size_classes.len())?)?;
    for class in &snapshot.size_classes {
        writer.write_u32(class.class_index)?;
        writer.write_u64(class.block_bytes)?;
        write_estimate(&mut writer, class.live_allocations)?;
        write_estimate(&mut writer, class.requested_bytes)?;
        write_estimate(&mut writer, class.usable_bytes)?;
    }

    let regions_len = checked_add(4, checked_mul(snapshot.regions.len(), 4 + 3 * 8)?)?;
    writer.begin_section(SECTION_REGIONS, SECTION_VERSION, regions_len)?;
    writer.write_u32(count(snapshot.regions.len())?)?;
    for region in &snapshot.regions {
        writer.write_u32(region.region_index)?;
        writer.write_u64(region.reserved_bytes)?;
        writer.write_u64(region.used_slices)?;
        writer.write_u64(region.free_slices)?;
    }

    let topology_len = topology_encoded_len(&snapshot.topology)?;
    writer.begin_section(SECTION_TOPOLOGY, TOPOLOGY_SECTION_VERSION, topology_len)?;
    write_topology(&mut writer, &snapshot.topology)?;

    let domains_len = domains_encoded_len(&snapshot.domains)?;
    writer.begin_section(SECTION_DOMAINS, SECTION_VERSION, domains_len)?;
    write_domains(&mut writer, &snapshot.domains)?;

    let callers_len = callers_encoded_len(snapshot.callers.as_ref())?;
    writer.begin_section(SECTION_CALLERS, CALLERS_SECTION_VERSION, callers_len)?;
    write_callers(&mut writer, snapshot.callers.as_ref())?;

    let histograms_len = histograms_encoded_len(&snapshot.histograms)?;
    writer.begin_section(SECTION_HISTOGRAMS, SECTION_VERSION, histograms_len)?;
    write_histograms(&mut writer, &snapshot.histograms)?;

    let addresses_len = addresses_encoded_len(&snapshot.addresses)?;
    writer.begin_section(SECTION_ADDRESSES, SECTION_VERSION, addresses_len)?;
    write_addresses(&mut writer, &snapshot.addresses)?;
    writer.finish().map_err(Error::from)
}

/// Decodes a snapshot from wire bytes.
pub fn decode(bytes: &[u8]) -> Result<Snapshot, Error> {
    let mut reader = Reader::new(bytes);
    let header = reader.read_header()?;
    if header.telemetry_schema() == 0 || header.telemetry_schema() > TELEMETRY_SCHEMA_VERSION {
        return Err(Error::unsupported_schema(header.telemetry_schema()));
    }

    let mut snapshot = Snapshot::new(header.producer());
    snapshot.metadata.wire_format_version = header.wire_format();
    snapshot.metadata.telemetry_schema_version = header.telemetry_schema();
    let mut has_metadata = false;
    let mut has_stats = false;
    let mut seen_sections = 0_u16;

    while let Some(section) = reader.read_section()? {
        let bit = section.id().checked_sub(1).and_then(|shift| 1_u16.checked_shl(u32::from(shift)));
        if let Some(bit) = bit {
            if seen_sections & bit != 0 {
                return Err(Error::duplicate_section(section.id()));
            }
            seen_sections |= bit;
        }
        if section.id() == SECTION_TOPOLOGY {
            if section.version() != SECTION_VERSION && section.version() != TOPOLOGY_SECTION_VERSION {
                snapshot
                    .skipped_sections
                    .push(snapshot::SkippedSection::from_fields(SkippedSectionFields {
                        id: section.id(),
                        version: section.version(),
                    }));
                continue;
            }
        } else if section.id() == SECTION_CALLERS {
            if !(SECTION_VERSION..=CALLERS_SECTION_VERSION).contains(&section.version()) {
                snapshot
                    .skipped_sections
                    .push(snapshot::SkippedSection::from_fields(SkippedSectionFields {
                        id: section.id(),
                        version: section.version(),
                    }));
                continue;
            }
        } else if section.version() != SECTION_VERSION {
            snapshot
                .skipped_sections
                .push(snapshot::SkippedSection::from_fields(SkippedSectionFields {
                    id: section.id(),
                    version: section.version(),
                }));
            continue;
        }
        let mut payload = Reader::new(section.payload());
        match section.id() {
            SECTION_METADATA => {
                snapshot.metadata.capture_duration_nanos = payload.read_u64()?;
                has_metadata = true;
            }
            SECTION_STATS => {
                snapshot.stats = read_stats(&mut payload)?;
                has_stats = true;
            }
            SECTION_SIZE_CLASSES => snapshot.size_classes = read_size_classes(&mut payload)?,
            SECTION_REGIONS => snapshot.regions = read_regions(&mut payload)?,
            SECTION_TOPOLOGY => snapshot.topology = read_topology(&mut payload, section.version())?,
            SECTION_DOMAINS => snapshot.domains = read_domains(&mut payload)?,
            SECTION_CALLERS => snapshot.callers = read_callers(&mut payload, section.version())?,
            SECTION_HISTOGRAMS => snapshot.histograms = read_histograms(&mut payload)?,
            SECTION_ADDRESSES => snapshot.addresses = read_addresses(&mut payload)?,
            _ => {
                snapshot
                    .skipped_sections
                    .push(snapshot::SkippedSection::from_fields(SkippedSectionFields {
                        id: section.id(),
                        version: section.version(),
                    }));
                continue;
            }
        }
        if payload.remaining() != 0 {
            return Err(Error::malformed_section(section.id()));
        }
    }
    if !has_metadata {
        return Err(Error::missing_section(SECTION_METADATA));
    }
    if !has_stats {
        return Err(Error::missing_section(SECTION_STATS));
    }
    Ok(snapshot)
}

impl Encode for Snapshot {
    type Error = Error;

    fn encoded_len(&self) -> Result<usize, Self::Error> {
        encoded_len(self)
    }

    fn encode(&self, output: &mut [u8]) -> Result<usize, Self::Error> {
        encode(self, output)
    }
}

impl Decode for Snapshot {
    type Error = Error;

    fn decode(input: &[u8]) -> Result<Self, Self::Error> {
        decode(input)
    }
}

fn write_stats(writer: &mut Writer<'_>, stats: Stats) -> Result<(), Error> {
    for value in [
        stats.allocated_bytes,
        stats.deallocated_bytes,
        stats.live_bytes,
        stats.peak_live_bytes,
        stats.mapped_bytes,
        stats.os_mappings,
        stats.os_unmappings,
        stats.allocations,
        stats.deallocations,
        stats.remote_frees,
        stats.pending_remote_blocks,
        stats.remote_pushes_in_progress,
        stats.drained_remote_blocks,
    ] {
        writer.write_u64(value)?;
    }
    Ok(())
}

fn read_stats(reader: &mut Reader<'_>) -> Result<Stats, Error> {
    Ok(Stats {
        allocated_bytes: reader.read_u64()?,
        deallocated_bytes: reader.read_u64()?,
        live_bytes: reader.read_u64()?,
        peak_live_bytes: reader.read_u64()?,
        mapped_bytes: reader.read_u64()?,
        os_mappings: reader.read_u64()?,
        os_unmappings: reader.read_u64()?,
        allocations: reader.read_u64()?,
        deallocations: reader.read_u64()?,
        remote_frees: reader.read_u64()?,
        pending_remote_blocks: reader.read_u64()?,
        remote_pushes_in_progress: reader.read_u64()?,
        drained_remote_blocks: reader.read_u64()?,
    })
}

fn write_estimate(writer: &mut Writer<'_>, estimate: Estimate) -> Result<(), Error> {
    writer.write_u64(estimate.value)?;
    writer.write_u64(estimate.lower_bound)?;
    writer.write_u64(estimate.upper_bound)?;
    Ok(())
}

fn read_estimate(reader: &mut Reader<'_>) -> Result<Estimate, Error> {
    Ok(Estimate {
        value: reader.read_u64()?,
        lower_bound: reader.read_u64()?,
        upper_bound: reader.read_u64()?,
    })
}

fn read_size_classes(reader: &mut Reader<'_>) -> Result<Vec<SizeClass>, Error> {
    let count = usize_count(reader.read_u32()?)?;
    if count > reader.remaining() / (4 + 8 + 9 * 8) {
        return Err(Error::malformed_section(SECTION_SIZE_CLASSES));
    }
    let mut classes = Vec::with_capacity(count);
    for _ in 0..count {
        classes.push(SizeClass {
            class_index: reader.read_u32()?,
            block_bytes: reader.read_u64()?,
            live_allocations: read_estimate(reader)?,
            requested_bytes: read_estimate(reader)?,
            usable_bytes: read_estimate(reader)?,
        });
    }
    Ok(classes)
}

fn read_regions(reader: &mut Reader<'_>) -> Result<Vec<Region>, Error> {
    let count = usize_count(reader.read_u32()?)?;
    if count > reader.remaining() / (4 + 3 * 8) {
        return Err(Error::malformed_section(SECTION_REGIONS));
    }
    let mut regions = Vec::with_capacity(count);
    for _ in 0..count {
        regions.push(Region {
            region_index: reader.read_u32()?,
            reserved_bytes: reader.read_u64()?,
            used_slices: reader.read_u64()?,
            free_slices: reader.read_u64()?,
        });
    }
    Ok(regions)
}

fn domains_encoded_len(domains: &[Domain]) -> Result<usize, Error> {
    count(domains.len())?;
    let mut length = 4;
    for domain in domains {
        count(domain.region_indices.len())?;
        length = checked_add(length, 8 + 1 + 8 * 8 + 4)?;
        length = checked_add(length, checked_mul(domain.region_indices.len(), 4)?)?;
    }
    Ok(length)
}

fn write_domains(writer: &mut Writer<'_>, domains: &[Domain]) -> Result<(), Error> {
    writer.write_u32(count(domains.len())?)?;
    for domain in domains {
        writer.write_u64(domain.domain_id)?;
        writer.write_u8(u8::from(domain.is_default))?;
        for value in [
            domain.region_count,
            domain.reserved_bytes,
            domain.used_slices,
            domain.free_slices,
            domain.small_slices,
            domain.medium_slices,
            domain.bump_slices,
            domain.unknown_slices,
        ] {
            writer.write_u64(value)?;
        }
        writer.write_u32(count(domain.region_indices.len())?)?;
        for region_index in &domain.region_indices {
            writer.write_u32(*region_index)?;
        }
    }
    Ok(())
}

fn read_domains(reader: &mut Reader<'_>) -> Result<Vec<Domain>, Error> {
    let count = usize_count(reader.read_u32()?)?;
    if count > reader.remaining() / (8 + 1 + 8 * 8 + 4) {
        return Err(Error::malformed_section(SECTION_DOMAINS));
    }
    let mut domains = Vec::with_capacity(count);
    for _ in 0..count {
        let domain_id = reader.read_u64()?;
        let is_default = read_bool(reader, SECTION_DOMAINS)?;
        let region_count = reader.read_u64()?;
        let reserved_bytes = reader.read_u64()?;
        let used_slices = reader.read_u64()?;
        let free_slices = reader.read_u64()?;
        let small_slices = reader.read_u64()?;
        let medium_slices = reader.read_u64()?;
        let bump_slices = reader.read_u64()?;
        let unknown_slices = reader.read_u64()?;
        let region_index_count = usize_count(reader.read_u32()?)?;
        if region_index_count > reader.remaining() / 4 {
            return Err(Error::malformed_section(SECTION_DOMAINS));
        }
        let mut region_indices = Vec::with_capacity(region_index_count);
        for _ in 0..region_index_count {
            region_indices.push(reader.read_u32()?);
        }
        domains.push(Domain {
            domain_id,
            is_default,
            region_count,
            reserved_bytes,
            used_slices,
            free_slices,
            small_slices,
            medium_slices,
            bump_slices,
            unknown_slices,
            region_indices,
        });
    }
    Ok(domains)
}

fn topology_encoded_len(regions: &[TopologyRegion]) -> Result<usize, Error> {
    count(regions.len())?;
    let mut length = 4;
    for region in regions {
        count(region.used_bitmap.len())?;
        count(region.slices.len())?;
        length = checked_add(length, 4 + 3 * 8 + 4)?;
        length = checked_add(length, checked_mul(region.used_bitmap.len(), 8)?)?;
        length = checked_add(length, 4)?;
        for slice in &region.slices {
            u8::try_from(slice.segments.len()).map_err(|_| Error::LENGTH_OVERFLOW)?;
            length = checked_add(length, 4 + 1 + 4 + 3 * 8 + 1)?;
            let segments_len = checked_mul(slice.segments.len(), 1 + 4 + 1 + 4 + 4 + 1)?;
            length = checked_add(length, segments_len)?;
        }
    }
    Ok(length)
}

fn write_topology(writer: &mut Writer<'_>, regions: &[TopologyRegion]) -> Result<(), Error> {
    writer.write_u32(count(regions.len())?)?;
    for region in regions {
        writer.write_u32(region.region_index)?;
        writer.write_u64(region.base_address)?;
        writer.write_u64(region.region_bytes)?;
        writer.write_u64(region.slice_bytes)?;
        writer.write_u32(count(region.used_bitmap.len())?)?;
        for word in &region.used_bitmap {
            writer.write_u64(*word)?;
        }
        writer.write_u32(count(region.slices.len())?)?;
        for slice in &region.slices {
            writer.write_u32(slice.slice_index)?;
            match slice.kind {
                SliceKind::Unknown => writer.write_u8(0)?,
                SliceKind::Small => writer.write_u8(1)?,
                SliceKind::Medium => writer.write_u8(2)?,
                SliceKind::MediumContinuation => writer.write_u8(3)?,
                SliceKind::Bump => writer.write_u8(4)?,
            }
            writer.write_u32(slice.span_slices)?;
            writer.write_u64(slice.owner)?;
            writer.write_u64(slice.requested_bytes)?;
            writer.write_u64(slice.usable_bytes)?;
            let segment_count = u8::try_from(slice.segments.len()).map_err(|_| Error::LENGTH_OVERFLOW)?;
            writer.write_u8(segment_count)?;
            for segment in &slice.segments {
                writer.write_u8(segment.segment_index)?;
                writer.write_u32(segment.class_index)?;
                writer.write_u8(u8::from(segment.context))?;
                writer.write_u32(segment.live_blocks)?;
                writer.write_u32(segment.usable_blocks)?;
                writer.write_u8(u8::from(segment.utilization_tracked))?;
            }
        }
    }
    Ok(())
}

fn read_topology(reader: &mut Reader<'_>, section_version: u16) -> Result<Vec<TopologyRegion>, Error> {
    let count = usize_count(reader.read_u32()?)?;
    if count > reader.remaining() / (4 + 3 * 8 + 4 + 4) {
        return Err(Error::malformed_section(SECTION_TOPOLOGY));
    }
    let mut regions = Vec::with_capacity(count);
    for _ in 0..count {
        let region_index = reader.read_u32()?;
        let base_address = reader.read_u64()?;
        let region_bytes = reader.read_u64()?;
        let slice_bytes = reader.read_u64()?;
        if region_bytes == 0
            || slice_bytes == 0
            || !slice_bytes.is_power_of_two()
            || !base_address.is_multiple_of(slice_bytes)
            || !region_bytes.is_multiple_of(slice_bytes)
        {
            return Err(Error::malformed_section(SECTION_TOPOLOGY));
        }
        let total_slices = region_bytes / slice_bytes;
        let expected_bitmap_count = usize::try_from(total_slices.div_ceil(64)).map_err(|_| Error::malformed_section(SECTION_TOPOLOGY))?;
        let bitmap_count = usize_count(reader.read_u32()?)?;
        if bitmap_count != expected_bitmap_count || bitmap_count > reader.remaining() / 8 {
            return Err(Error::malformed_section(SECTION_TOPOLOGY));
        }
        let mut used_bitmap = Vec::with_capacity(bitmap_count);
        for _ in 0..bitmap_count {
            used_bitmap.push(reader.read_u64()?);
        }
        let trailing_slices = total_slices % 64;
        if trailing_slices != 0 && used_bitmap.last().is_some_and(|word| word >> trailing_slices != 0) {
            return Err(Error::malformed_section(SECTION_TOPOLOGY));
        }
        let slice_count = usize_count(reader.read_u32()?)?;
        let used_slice_count = used_bitmap.iter().try_fold(0_usize, |total, word| {
            total
                .checked_add(word.count_ones() as usize)
                .ok_or_else(|| Error::malformed_section(SECTION_TOPOLOGY))
        })?;
        if slice_count != used_slice_count || slice_count > reader.remaining() / (4 + 1 + 4 + 3 * 8 + 1) {
            return Err(Error::malformed_section(SECTION_TOPOLOGY));
        }
        let mut slices = Vec::with_capacity(slice_count);
        let mut detailed_bitmap = vec![0_u64; bitmap_count];
        for _ in 0..slice_count {
            let slice_index = reader.read_u32()?;
            if u64::from(slice_index) >= total_slices {
                return Err(Error::malformed_section(SECTION_TOPOLOGY));
            }
            let slice_offset = usize::try_from(slice_index).map_err(|_| Error::malformed_section(SECTION_TOPOLOGY))?;
            let word_index = slice_offset / 64;
            let slice_bit = 1_u64 << (slice_offset % 64);
            if used_bitmap[word_index] & slice_bit == 0 || detailed_bitmap[word_index] & slice_bit != 0 {
                return Err(Error::malformed_section(SECTION_TOPOLOGY));
            }
            detailed_bitmap[word_index] |= slice_bit;
            let kind = match reader.read_u8()? {
                0 => SliceKind::Unknown,
                1 => SliceKind::Small,
                2 => SliceKind::Medium,
                3 => SliceKind::MediumContinuation,
                4 => SliceKind::Bump,
                value => return Err(Error::unknown_slice_kind(value)),
            };
            let span_slices = reader.read_u32()?;
            let owner = reader.read_u64()?;
            let requested_bytes = reader.read_u64()?;
            let usable_bytes = reader.read_u64()?;
            let segment_count = usize::from(reader.read_u8()?);
            let segment_bytes = if section_version == SECTION_VERSION {
                1 + 4 + 1
            } else {
                1 + 4 + 1 + 4 + 4 + 1
            };
            if segment_count > reader.remaining() / segment_bytes {
                return Err(Error::malformed_section(SECTION_TOPOLOGY));
            }
            let mut segments = Vec::with_capacity(segment_count);
            for _ in 0..segment_count {
                let segment_index = reader.read_u8()?;
                let class_index = reader.read_u32()?;
                let context = read_bool(reader, SECTION_TOPOLOGY)?;
                let (live_blocks, usable_blocks, utilization_tracked) = if section_version == SECTION_VERSION {
                    (0, 0, false)
                } else {
                    (reader.read_u32()?, reader.read_u32()?, read_bool(reader, SECTION_TOPOLOGY)?)
                };
                segments.push(Segment {
                    segment_index,
                    class_index,
                    context,
                    live_blocks,
                    usable_blocks,
                    utilization_tracked,
                });
            }
            slices.push(Slice {
                slice_index,
                kind,
                span_slices,
                owner,
                requested_bytes,
                usable_bytes,
                segments,
            });
        }
        validate_detailed_bitmap(&detailed_bitmap, &used_bitmap)?;
        regions.push(TopologyRegion {
            region_index,
            base_address,
            region_bytes,
            slice_bytes,
            used_bitmap,
            slices,
        });
    }
    Ok(regions)
}

fn validate_detailed_bitmap(detailed_bitmap: &[u64], used_bitmap: &[u64]) -> Result<(), Error> {
    if detailed_bitmap != used_bitmap {
        return Err(Error::malformed_section(SECTION_TOPOLOGY));
    }
    Ok(())
}

fn callers_encoded_len(callers: Option<&Callers>) -> Result<usize, Error> {
    let Some(callers) = callers else {
        return Ok(1);
    };
    count(callers.threads.len())?;
    count(callers.events.len())?;
    count(callers.thread_names.len())?;
    let mut length = 1 + 3 * 8 + 4;
    for thread in &callers.threads {
        count(thread.allocated_histogram.len())?;
        count(thread.live_histogram.len())?;
        length = checked_add(length, 3 * 8 + 4 + checked_mul(thread.allocated_histogram.len(), 8)?)?;
        length = checked_add(length, 4 + checked_mul(thread.live_histogram.len(), 8)?)?;
    }
    let stacks = unique_call_stacks(callers);
    count(stacks.len())?;
    length = checked_add(length, 4)?;
    for stack in stacks {
        count(stack.len())?;
        length = checked_add(length, 4 + checked_mul(stack.len(), 8)?)?;
    }
    length = checked_add(length, 4)?;
    length = checked_add(length, checked_mul(callers.events.len(), 8 * 8 + 4 + 3)?)?;
    length = checked_add(length, 4)?;
    for thread in &callers.thread_names {
        count(thread.name.len())?;
        length = checked_add(length, 8 + 4 + thread.name.len())?;
    }
    Ok(length)
}

fn unique_call_stacks(callers: &Callers) -> Vec<&[u64]> {
    let mut stacks = Vec::new();
    let mut seen = HashSet::new();
    for event in &callers.events {
        if seen.insert(event.call_stack.as_slice()) {
            stacks.push(event.call_stack.as_slice());
        }
    }
    stacks
}

fn write_callers(writer: &mut Writer<'_>, callers: Option<&Callers>) -> Result<(), Error> {
    let Some(callers) = callers else {
        writer.write_u8(0)?;
        return Ok(());
    };
    writer.write_u8(1)?;
    writer.write_u64(callers.session_id)?;
    writer.write_u64(callers.total_events)?;
    writer.write_u64(callers.lost_events)?;
    writer.write_u32(count(callers.threads.len())?)?;
    for thread in &callers.threads {
        writer.write_u64(thread.thread_log_id)?;
        writer.write_u64(thread.total_events)?;
        writer.write_u64(thread.lost_events)?;
        write_histogram(writer, &thread.allocated_histogram)?;
        write_histogram(writer, &thread.live_histogram)?;
    }
    let stacks = unique_call_stacks(callers);
    let stack_indexes = stacks
        .iter()
        .enumerate()
        .map(|(index, &stack)| (stack, index))
        .collect::<HashMap<_, _>>();
    writer.write_u32(count(stacks.len())?)?;
    for stack in &stacks {
        writer.write_u32(count(stack.len())?)?;
        for &instruction_pointer in *stack {
            writer.write_u64(instruction_pointer)?;
        }
    }
    writer.write_u32(count(callers.events.len())?)?;
    for event in &callers.events {
        writer.write_u64(event.thread_log_id)?;
        writer.write_u64(event.event_thread_id)?;
        writer.write_u64(event.sequence)?;
        writer.write_u64(event.allocation_id)?;
        match event.kind {
            EventKind::Allocated => writer.write_u8(1)?,
            EventKind::Deallocated => writer.write_u8(2)?,
        }
        writer.write_u64(event.heap_id)?;
        writer.write_u8(match event.heap_kind {
            HeapKind::General => 1,
            HeapKind::Bump => 2,
            HeapKind::Thread => 3,
        })?;
        writer.write_u8(u8::from(event.freed_after_heap_release))?;
        writer.write_u64(event.address)?;
        writer.write_u64(event.size)?;
        writer.write_u64(event.align)?;
        let stack_index = stack_indexes[event.call_stack.as_slice()];
        writer.write_u32(count(stack_index)?)?;
    }
    writer.write_u32(count(callers.thread_names.len())?)?;
    for thread in &callers.thread_names {
        writer.write_u64(thread.thread_id)?;
        write_string(writer, &thread.name)?;
    }
    Ok(())
}

fn read_callers(reader: &mut Reader<'_>, version: u16) -> Result<Option<Callers>, Error> {
    let expanded_frame_limit = reader.remaining() / 4;
    let mut expanded_frames = 0_usize;
    match reader.read_u8()? {
        0 => return Ok(None),
        1 => {}
        _ => return Err(Error::malformed_section(SECTION_CALLERS)),
    }
    let session_id = reader.read_u64()?;
    let total_events = reader.read_u64()?;
    let lost_events = reader.read_u64()?;
    let thread_count = usize_count(reader.read_u32()?)?;
    if thread_count > reader.remaining() / (3 * 8) {
        return Err(Error::malformed_section(SECTION_CALLERS));
    }
    let mut threads = Vec::with_capacity(thread_count);
    for _ in 0..thread_count {
        threads.push(ThreadLog {
            thread_log_id: reader.read_u64()?,
            total_events: reader.read_u64()?,
            lost_events: reader.read_u64()?,
            allocated_histogram: if version >= CALLERS_DIAGNOSTICS_VERSION {
                read_histogram(reader, SECTION_CALLERS)?
            } else {
                Vec::new()
            },
            live_histogram: if version >= CALLERS_DIAGNOSTICS_VERSION {
                read_histogram(reader, SECTION_CALLERS)?
            } else {
                Vec::new()
            },
        });
    }
    let stacks = if version >= CALLERS_STACK_TABLE_VERSION {
        let stack_count = usize_count(reader.read_u32()?)?;
        if stack_count > reader.remaining() / 4 {
            return Err(Error::malformed_section(SECTION_CALLERS));
        }
        let mut stacks = Vec::with_capacity(stack_count);
        for _ in 0..stack_count {
            let frame_count = usize_count(reader.read_u32()?)?;
            if frame_count > reader.remaining() / 8 {
                return Err(Error::malformed_section(SECTION_CALLERS));
            }
            let mut stack = Vec::with_capacity(frame_count);
            for _ in 0..frame_count {
                stack.push(reader.read_u64()?);
            }
            stacks.push(stack);
        }
        stacks
    } else {
        Vec::new()
    };
    let event_count = usize_count(reader.read_u32()?)?;
    let minimum_event_bytes = if version >= CALLERS_DIAGNOSTICS_VERSION {
        8 * 8 + 4 + 3
    } else {
        6 * 8 + 4 + 1
    };
    if event_count > reader.remaining() / minimum_event_bytes {
        return Err(Error::malformed_section(SECTION_CALLERS));
    }
    let mut events = Vec::with_capacity(event_count);
    for _ in 0..event_count {
        let thread_log_id = reader.read_u64()?;
        let event_thread_id = if version >= CALLERS_DIAGNOSTICS_VERSION {
            reader.read_u64()?
        } else {
            thread_log_id
        };
        let sequence = reader.read_u64()?;
        let allocation_id = reader.read_u64()?;
        let kind = match reader.read_u8()? {
            1 => EventKind::Allocated,
            2 => EventKind::Deallocated,
            value => return Err(Error::unknown_event_kind(value)),
        };
        let heap_id = if version >= CALLERS_DIAGNOSTICS_VERSION {
            reader.read_u64()?
        } else {
            0
        };
        let heap_kind = if version >= CALLERS_DIAGNOSTICS_VERSION {
            match reader.read_u8()? {
                1 => HeapKind::General,
                2 => HeapKind::Bump,
                3 => HeapKind::Thread,
                _ => return Err(Error::malformed_section(SECTION_CALLERS)),
            }
        } else {
            HeapKind::General
        };
        let freed_after_heap_release = if version >= CALLERS_DIAGNOSTICS_VERSION {
            read_bool(reader, SECTION_CALLERS)?
        } else {
            false
        };
        let address = reader.read_u64()?;
        let size = reader.read_u64()?;
        let align = reader.read_u64()?;
        if address == 0 || align == 0 || !align.is_power_of_two() {
            return Err(Error::malformed_section(SECTION_CALLERS));
        }
        let call_stack = if version >= CALLERS_STACK_TABLE_VERSION {
            let stack_index = usize_count(reader.read_u32()?)?;
            stacks
                .get(stack_index)
                .cloned()
                .ok_or_else(|| Error::malformed_section(SECTION_CALLERS))?
        } else {
            let frame_count = usize_count(reader.read_u32()?)?;
            if frame_count > reader.remaining() / 8 {
                return Err(Error::malformed_section(SECTION_CALLERS));
            }
            let mut call_stack = Vec::with_capacity(frame_count);
            for _ in 0..frame_count {
                call_stack.push(reader.read_u64()?);
            }
            call_stack
        };
        expanded_frames = expanded_frames.saturating_add(call_stack.len());
        if expanded_frames > expanded_frame_limit {
            return Err(Error::malformed_section(SECTION_CALLERS));
        }
        events.push(Event {
            thread_log_id,
            event_thread_id,
            sequence,
            allocation_id,
            kind,
            heap_id,
            heap_kind,
            freed_after_heap_release,
            address,
            size,
            align,
            call_stack,
        });
    }
    let thread_names = if version >= CALLERS_THREAD_NAMES_VERSION {
        let count = usize_count(reader.read_u32()?)?;
        if count > reader.remaining() / (8 + 4) {
            return Err(Error::malformed_section(SECTION_CALLERS));
        }
        let mut names = Vec::with_capacity(count);
        for _ in 0..count {
            names.push(ThreadName {
                thread_id: reader.read_u64()?,
                name: read_string_in_section(reader, SECTION_CALLERS)?,
            });
        }
        names
    } else {
        Vec::new()
    };
    Ok(Some(Callers {
        session_id,
        total_events,
        lost_events,
        threads,
        events,
        thread_names,
    }))
}

fn histograms_encoded_len(histograms: &Histograms) -> Result<usize, Error> {
    count(histograms.allocated.len())?;
    count(histograms.live.len())?;
    checked_add(
        checked_add(4, checked_mul(histograms.allocated.len(), 8)?)?,
        checked_add(4, checked_mul(histograms.live.len(), 8)?)?,
    )
}

fn write_histograms(writer: &mut Writer<'_>, histograms: &Histograms) -> Result<(), Error> {
    write_histogram(writer, &histograms.allocated)?;
    write_histogram(writer, &histograms.live)
}

fn read_histograms(reader: &mut Reader<'_>) -> Result<Histograms, Error> {
    Ok(Histograms {
        allocated: read_histogram(reader, SECTION_HISTOGRAMS)?,
        live: read_histogram(reader, SECTION_HISTOGRAMS)?,
    })
}

fn write_histogram(writer: &mut Writer<'_>, histogram: &[u64]) -> Result<(), Error> {
    writer.write_u32(count(histogram.len())?)?;
    for &count in histogram {
        writer.write_u64(count)?;
    }
    Ok(())
}

fn read_histogram(reader: &mut Reader<'_>, section_id: u16) -> Result<Vec<u64>, Error> {
    let count = usize_count(reader.read_u32()?)?;
    if count > reader.remaining() / 8 {
        return Err(Error::malformed_section(section_id));
    }
    let mut histogram = Vec::with_capacity(count);
    for _ in 0..count {
        histogram.push(reader.read_u64()?);
    }
    Ok(histogram)
}

fn addresses_encoded_len(addresses: &[AddressLookup]) -> Result<usize, Error> {
    count(addresses.len())?;
    let mut length = 4;
    for lookup in addresses {
        length = checked_add(length, 8 + 1)?;
        if let Some(symbol) = &lookup.symbol {
            length = checked_add(length, checked_add(4, symbol.len())?)?;
        }
        if let Some(filename) = &lookup.filename {
            length = checked_add(length, checked_add(4, filename.len())?)?;
        }
        if lookup.line.is_some() {
            length = checked_add(length, 4)?;
        }
        if lookup.column.is_some() {
            length = checked_add(length, 4)?;
        }
    }
    Ok(length)
}

fn write_addresses(writer: &mut Writer<'_>, addresses: &[AddressLookup]) -> Result<(), Error> {
    writer.write_u32(count(addresses.len())?)?;
    for lookup in addresses {
        writer.write_u64(lookup.address)?;
        let flags = u8::from(lookup.symbol.is_some())
            | (u8::from(lookup.filename.is_some()) << 1)
            | (u8::from(lookup.line.is_some()) << 2)
            | (u8::from(lookup.column.is_some()) << 3);
        writer.write_u8(flags)?;
        if let Some(symbol) = &lookup.symbol {
            write_string(writer, symbol)?;
        }
        if let Some(filename) = &lookup.filename {
            write_string(writer, filename)?;
        }
        if let Some(line) = lookup.line {
            writer.write_u32(line)?;
        }
        if let Some(column) = lookup.column {
            writer.write_u32(column)?;
        }
    }
    Ok(())
}

fn read_addresses(reader: &mut Reader<'_>) -> Result<Vec<AddressLookup>, Error> {
    let count = usize_count(reader.read_u32()?)?;
    if count > reader.remaining() / 9 {
        return Err(Error::malformed_section(SECTION_ADDRESSES));
    }
    let mut addresses = Vec::with_capacity(count);
    for _ in 0..count {
        let address = reader.read_u64()?;
        let flags = reader.read_u8()?;
        if flags & !0x0F != 0 {
            return Err(Error::malformed_section(SECTION_ADDRESSES));
        }
        addresses.push(AddressLookup {
            address,
            symbol: (flags & 1 != 0).then(|| read_string(reader)).transpose()?,
            filename: (flags & 2 != 0).then(|| read_string(reader)).transpose()?,
            line: (flags & 4 != 0).then(|| reader.read_u32()).transpose()?,
            column: (flags & 8 != 0).then(|| reader.read_u32()).transpose()?,
        });
    }
    Ok(addresses)
}

fn write_string(writer: &mut Writer<'_>, value: &str) -> Result<(), Error> {
    writer.write_u32(count(value.len())?)?;
    writer.write_bytes(value.as_bytes())?;
    Ok(())
}

fn read_string(reader: &mut Reader<'_>) -> Result<String, Error> {
    read_string_in_section(reader, SECTION_ADDRESSES)
}

fn read_string_in_section(reader: &mut Reader<'_>, section_id: u16) -> Result<String, Error> {
    let length = usize_count(reader.read_u32()?)?;
    let bytes = reader.read_bytes(length)?;
    String::from_utf8(bytes.to_vec()).map_err(|_| Error::invalid_utf8(section_id))
}

fn read_bool(reader: &mut Reader<'_>, section_id: u16) -> Result<bool, Error> {
    match reader.read_u8()? {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(Error::malformed_section(section_id)),
    }
}

fn count(value: usize) -> Result<u32, Error> {
    u32::try_from(value).map_err(|_| Error::LENGTH_OVERFLOW)
}

fn usize_count(value: u32) -> Result<usize, Error> {
    usize::try_from(value).map_err(|_| Error::INTEGER_OVERFLOW)
}

fn checked_add(left: usize, right: usize) -> Result<usize, Error> {
    left.checked_add(right).ok_or(Error::LENGTH_OVERFLOW)
}

fn checked_mul(left: usize, right: usize) -> Result<usize, Error> {
    left.checked_mul(right).ok_or(Error::LENGTH_OVERFLOW)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_have_descriptive_output_and_sources() {
        let wire_source = Reader::new(&[]).read_u8().unwrap_err();
        let cases = [
            (Error::LENGTH_OVERFLOW, "the encoded snapshot length cannot be represented"),
            (Error::OUTPUT_TOO_SMALL, "the snapshot output buffer is too small"),
            (
                Error::OUTPUT_LENGTH_MISMATCH,
                "the snapshot output buffer must have the exact encoded length",
            ),
            (
                Error::wire(wire_source),
                "invalid wire container: the input or output ended unexpectedly",
            ),
            (Error::unsupported_schema(2), "telemetry schema version 2 is unsupported"),
            (Error::missing_section(3), "required telemetry section 3 is missing"),
            (Error::duplicate_section(4), "telemetry section 4 appears more than once"),
            (Error::malformed_section(5), "telemetry section 5 is malformed"),
            (Error::INTEGER_OVERFLOW, "a decoded integer cannot fit in the target type"),
            (Error::invalid_utf8(6), "telemetry section 6 contains invalid UTF-8"),
            (Error::unknown_event_kind(7), "event kind 7 is unknown"),
            (Error::unknown_slice_kind(8), "slice kind 8 is unknown"),
        ];

        for (error, message) in cases {
            assert_eq!(error.to_string(), message);
            assert_eq!(format!("{error:?}"), format!("Error({message})"));
        }
        assert_eq!(
            Error::malformed_section(SECTION_CALLERS).kind(),
            ErrorKind::MalformedSection(SECTION_CALLERS)
        );

        assert!(std::error::Error::source(&Error::wire(wire_source)).is_some());
        assert!(std::error::Error::source(&Error::LENGTH_OVERFLOW).is_none());
    }

    #[test]
    fn invalid_callers_presence_flag_is_a_malformed_section() {
        let error = read_callers(&mut Reader::new(&[2]), CALLERS_SECTION_VERSION).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::MalformedSection(SECTION_CALLERS));
    }

    #[test]
    fn detailed_topology_bitmap_must_match_used_bitmap() {
        validate_detailed_bitmap(&[0b01], &[0b11]).unwrap_err();
    }

    #[test]
    fn caller_heap_kind_write_errors_are_propagated() {
        let callers = Callers {
            events: vec![Event::default()],
            ..Callers::default()
        };
        let mut bytes = [0_u8; 82];
        write_callers(&mut Writer::new(&mut bytes), Some(&callers)).unwrap_err();
    }

    #[test]
    fn caller_stack_table_deduplicates_equal_stacks() {
        let callers = Callers {
            events: vec![
                Event {
                    call_stack: vec![1, 2, 3],
                    ..Event::default()
                },
                Event {
                    call_stack: vec![1, 2, 3],
                    ..Event::default()
                },
                Event {
                    call_stack: vec![4, 5],
                    ..Event::default()
                },
            ],
            ..Callers::default()
        };

        assert_eq!(unique_call_stacks(&callers), [&[1, 2, 3][..], &[4, 5][..]]);
    }

    #[test]
    fn caller_stack_references_cannot_amplify_without_bound() {
        let mut payload = Vec::new();
        payload.push(1);
        payload.extend_from_slice(&1_u64.to_le_bytes());
        payload.extend_from_slice(&100_u64.to_le_bytes());
        payload.extend_from_slice(&0_u64.to_le_bytes());
        payload.extend_from_slice(&0_u32.to_le_bytes());
        payload.extend_from_slice(&1_u32.to_le_bytes());
        payload.extend_from_slice(&64_u32.to_le_bytes());
        for frame in 0..64_u64 {
            payload.extend_from_slice(&frame.to_le_bytes());
        }
        payload.extend_from_slice(&100_u32.to_le_bytes());
        for sequence in 0..100_u64 {
            payload.extend_from_slice(&1_u64.to_le_bytes());
            payload.extend_from_slice(&1_u64.to_le_bytes());
            payload.extend_from_slice(&sequence.to_le_bytes());
            payload.extend_from_slice(&sequence.to_le_bytes());
            payload.push(1);
            payload.extend_from_slice(&1_u64.to_le_bytes());
            payload.push(1);
            payload.push(0);
            payload.extend_from_slice(&1_u64.to_le_bytes());
            payload.extend_from_slice(&1_u64.to_le_bytes());
            payload.extend_from_slice(&1_u64.to_le_bytes());
            payload.extend_from_slice(&0_u32.to_le_bytes());
        }

        let error = read_callers(&mut Reader::new(&payload), CALLERS_STACK_TABLE_VERSION).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::MalformedSection(SECTION_CALLERS));
    }

    #[test]
    fn caller_stack_table_rejects_malformed_counts_and_references() {
        fn prefix() -> Vec<u8> {
            let mut payload = Vec::new();
            payload.push(1);
            payload.extend_from_slice(&0_u64.to_le_bytes());
            payload.extend_from_slice(&0_u64.to_le_bytes());
            payload.extend_from_slice(&0_u64.to_le_bytes());
            payload.extend_from_slice(&0_u32.to_le_bytes());
            payload
        }

        let mut excessive_stacks = prefix();
        excessive_stacks.extend_from_slice(&1_u32.to_le_bytes());

        let mut incomplete_stack = prefix();
        incomplete_stack.extend_from_slice(&1_u32.to_le_bytes());
        incomplete_stack.extend_from_slice(&1_u32.to_le_bytes());

        let mut excessive_events = prefix();
        excessive_events.extend_from_slice(&0_u32.to_le_bytes());
        excessive_events.extend_from_slice(&1_u32.to_le_bytes());

        let mut invalid_stack = prefix();
        invalid_stack.extend_from_slice(&0_u32.to_le_bytes());
        invalid_stack.extend_from_slice(&1_u32.to_le_bytes());
        invalid_stack.extend_from_slice(&1_u64.to_le_bytes());
        invalid_stack.extend_from_slice(&1_u64.to_le_bytes());
        invalid_stack.extend_from_slice(&0_u64.to_le_bytes());
        invalid_stack.extend_from_slice(&0_u64.to_le_bytes());
        invalid_stack.push(1);
        invalid_stack.extend_from_slice(&0_u64.to_le_bytes());
        invalid_stack.push(1);
        invalid_stack.push(0);
        invalid_stack.extend_from_slice(&1_u64.to_le_bytes());
        invalid_stack.extend_from_slice(&1_u64.to_le_bytes());
        invalid_stack.extend_from_slice(&1_u64.to_le_bytes());
        invalid_stack.extend_from_slice(&0_u32.to_le_bytes());

        let mut incomplete_legacy_stack = prefix();
        incomplete_legacy_stack.extend_from_slice(&1_u32.to_le_bytes());
        incomplete_legacy_stack.extend_from_slice(&1_u64.to_le_bytes());
        incomplete_legacy_stack.extend_from_slice(&1_u64.to_le_bytes());
        incomplete_legacy_stack.extend_from_slice(&0_u64.to_le_bytes());
        incomplete_legacy_stack.extend_from_slice(&0_u64.to_le_bytes());
        incomplete_legacy_stack.push(1);
        incomplete_legacy_stack.extend_from_slice(&0_u64.to_le_bytes());
        incomplete_legacy_stack.push(1);
        incomplete_legacy_stack.push(0);
        incomplete_legacy_stack.extend_from_slice(&1_u64.to_le_bytes());
        incomplete_legacy_stack.extend_from_slice(&1_u64.to_le_bytes());
        incomplete_legacy_stack.extend_from_slice(&1_u64.to_le_bytes());
        incomplete_legacy_stack.extend_from_slice(&1_u32.to_le_bytes());

        let mut excessive_thread_names = prefix();
        excessive_thread_names.extend_from_slice(&0_u32.to_le_bytes());
        excessive_thread_names.extend_from_slice(&0_u32.to_le_bytes());
        excessive_thread_names.extend_from_slice(&1_u32.to_le_bytes());

        for (payload, version) in [
            (excessive_stacks, CALLERS_STACK_TABLE_VERSION),
            (incomplete_stack, CALLERS_STACK_TABLE_VERSION),
            (excessive_events, CALLERS_STACK_TABLE_VERSION),
            (invalid_stack, CALLERS_STACK_TABLE_VERSION),
            (incomplete_legacy_stack, CALLERS_THREAD_NAMES_VERSION),
            (excessive_thread_names, CALLERS_STACK_TABLE_VERSION),
        ] {
            let error = read_callers(&mut Reader::new(&payload), version).unwrap_err();
            assert_eq!(error.kind(), ErrorKind::MalformedSection(SECTION_CALLERS));
        }
    }
}
