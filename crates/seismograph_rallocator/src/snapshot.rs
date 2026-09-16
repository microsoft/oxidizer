// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Rallocator snapshot model types.
//!
//! Statistics section version 2 identifies the raw peak value's scope through
//! [`PeakLiveBytesScope`]. Version 1 remains readable, but its peak is reported as
//! unavailable rather than retroactively treated as a lifetime peak. Readers
//! predating statistics version 2 must be upgraded to read newly encoded snapshots
//! because the statistics section is required.

/// Version of the snapshot producer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Version {
    /// Major producer version.
    pub major: u16,
    /// Minor producer version.
    pub minor: u16,
    /// Patch producer version.
    pub patch: u16,
}

impl Version {
    /// Creates a producer version.
    #[must_use]
    pub const fn new(major: u16, minor: u16, patch: u16) -> Self {
        Self { major, minor, patch }
    }
}

/// A bounded estimate with lower and upper bounds.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Estimate {
    /// Estimated value.
    pub value: u64,
    /// Inclusive lower bound.
    pub lower_bound: u64,
    /// Inclusive upper bound.
    pub upper_bound: u64,
}

#[doc(hidden)]
#[derive(Clone, Copy, Debug)]
pub struct EstimateFields {
    pub value: u64,
    pub lower_bound: u64,
    pub upper_bound: u64,
}

impl Estimate {
    #[doc(hidden)]
    #[must_use]
    pub fn from_fields(fields: EstimateFields) -> Self {
        let EstimateFields {
            value,
            lower_bound,
            upper_bound,
        } = fields;
        Self {
            value,
            lower_bound,
            upper_bound,
        }
    }
}

/// Collection scope of the raw [`Stats::peak_live_bytes`] value.
///
/// A maximum over occasional counter reads cannot recover a lifetime peak:
/// allocations may be created and freed between those reads. Historical statistics
/// sections did not identify the scope and decode as [`Self::Unavailable`].
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub enum PeakLiveBytesScope {
    /// No trustworthy interpretation of the raw peak value was supplied.
    #[default]
    Unavailable,
    /// Maximum of live-byte totals observed when aggregate statistics were queried.
    ///
    /// Queries read independent counters and are not transactional. This is neither
    /// a lifetime high-water mark nor a guaranteed bound on one.
    SnapshotSamples,
    /// Producer explicitly tracked the live requested-byte lifetime high-water mark.
    Lifetime,
}

impl std::fmt::Display for PeakLiveBytesScope {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Unavailable => "unavailable",
            Self::SnapshotSamples => "aggregate-query samples",
            Self::Lifetime => "lifetime",
        })
    }
}

/// Process-wide allocator counters for one linked allocator instance.
///
/// Cumulative counters are not reset at recorder-session boundaries. Independently
/// read counters do not form a transactional snapshot, and a final snapshot alone
/// cannot isolate the allocations performed by one workload or recording session.
/// The collection-start epoch and general counter availability are not encoded;
/// zero values alone do not prove that no allocator activity occurred.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Stats {
    /// Cumulative requested bytes allocated.
    pub allocated_bytes: u64,
    /// Cumulative requested bytes deallocated.
    pub deallocated_bytes: u64,
    /// Currently live requested bytes.
    pub live_bytes: u64,
    /// Raw high-water-mark value; interpret only through [`Self::peak_live_bytes_scope`].
    ///
    /// Use [`Self::lifetime_peak_live_bytes`] when a true lifetime peak is required.
    /// Old snapshots retain their numeric value here but do not acquire historical
    /// peak information merely by being decoded by a newer reader.
    pub peak_live_bytes: u64,
    /// Whether the raw peak is unavailable, an aggregate-query sample maximum, or a lifetime peak.
    pub peak_live_bytes_scope: PeakLiveBytesScope,
    /// Currently mapped allocator bytes.
    pub mapped_bytes: u64,
    /// Operating-system mappings performed.
    pub os_mappings: u64,
    /// Operating-system unmappings performed.
    pub os_unmappings: u64,
    /// Successful allocation operations.
    pub allocations: u64,
    /// Deallocation operations.
    pub deallocations: u64,
    /// Cumulative cross-thread deallocation operations reported by the allocator.
    ///
    /// Current rallocator producers fold relaxed counter shards; a concurrent
    /// observation need not represent a common-instant scalar total.
    pub remote_frees: u64,
    /// Remote returns not yet logically claimed for owner processing.
    ///
    /// This can include begun but unpublished returns. Normal slab drains claim
    /// a detached list before recycling its nodes; zero is not a quiescence proof.
    pub pending_remote_blocks: u64,
    /// Remote push operations in progress.
    ///
    /// Current rallocator producers fold relaxed per-actor gauges. This observes
    /// a distributed interval, not a common instant; zero is not a quiescence
    /// proof. After producers synchronize and stop, the gauges balance to zero.
    /// Heap-retirement admission is independent of this diagnostic value.
    pub remote_pushes_in_progress: u64,
    /// Cumulative remote blocks logically claimed by owners, not physical reclamation.
    ///
    /// Current rallocator producers fold relaxed counter shards independently
    /// of the other fields. Historical captures retain their original values;
    /// decoding does not establish a physical-recycling boundary.
    pub drained_remote_blocks: u64,
}

#[doc(hidden)]
#[derive(Clone, Copy, Debug)]
pub struct StatsFields {
    pub allocated_bytes: u64,
    pub deallocated_bytes: u64,
    pub live_bytes: u64,
    pub peak_live_bytes: u64,
    pub mapped_bytes: u64,
    pub os_mappings: u64,
    pub os_unmappings: u64,
    pub allocations: u64,
    pub deallocations: u64,
    pub remote_frees: u64,
    pub pending_remote_blocks: u64,
    pub remote_pushes_in_progress: u64,
    pub drained_remote_blocks: u64,
}

impl Stats {
    #[doc(hidden)]
    #[must_use]
    pub fn from_fields(fields: StatsFields) -> Self {
        let StatsFields {
            allocated_bytes,
            deallocated_bytes,
            live_bytes,
            peak_live_bytes,
            mapped_bytes,
            os_mappings,
            os_unmappings,
            allocations,
            deallocations,
            remote_frees,
            pending_remote_blocks,
            remote_pushes_in_progress,
            drained_remote_blocks,
        } = fields;
        Self {
            allocated_bytes,
            deallocated_bytes,
            live_bytes,
            peak_live_bytes,
            peak_live_bytes_scope: PeakLiveBytesScope::Unavailable,
            mapped_bytes,
            os_mappings,
            os_unmappings,
            allocations,
            deallocations,
            remote_frees,
            pending_remote_blocks,
            remote_pushes_in_progress,
            drained_remote_blocks,
        }
    }

    /// Returns a lifetime peak only when the producer explicitly supplied that scope.
    #[must_use]
    pub const fn lifetime_peak_live_bytes(&self) -> Option<u64> {
        match self.peak_live_bytes_scope {
            PeakLiveBytesScope::Lifetime => Some(self.peak_live_bytes),
            _ => None,
        }
    }

    /// Returns the maximum observed at aggregate queries, not a lifetime high-water mark.
    #[must_use]
    pub const fn sampled_peak_live_bytes(&self) -> Option<u64> {
        match self.peak_live_bytes_scope {
            PeakLiveBytesScope::SnapshotSamples => Some(self.peak_live_bytes),
            _ => None,
        }
    }
}

/// Statistics for one allocation size class.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SizeClass {
    /// Allocator-defined size-class index.
    pub class_index: u32,
    /// Usable block size in bytes.
    pub block_bytes: u64,
    /// Live allocation count estimate.
    pub live_allocations: Estimate,
    /// Requested-byte estimate.
    pub requested_bytes: Estimate,
    /// Usable-byte estimate.
    pub usable_bytes: Estimate,
}

#[doc(hidden)]
#[derive(Clone, Copy, Debug)]
pub struct SizeClassFields {
    pub class_index: u32,
    pub block_bytes: u64,
    pub live_allocations: Estimate,
    pub requested_bytes: Estimate,
    pub usable_bytes: Estimate,
}

impl SizeClass {
    #[doc(hidden)]
    #[must_use]
    pub fn from_fields(fields: SizeClassFields) -> Self {
        let SizeClassFields {
            class_index,
            block_bytes,
            live_allocations,
            requested_bytes,
            usable_bytes,
        } = fields;
        Self {
            class_index,
            block_bytes,
            live_allocations,
            requested_bytes,
            usable_bytes,
        }
    }
}

/// Aggregate state for one allocator region.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Region {
    /// Region index.
    pub region_index: u32,
    /// Reserved virtual bytes.
    pub reserved_bytes: u64,
    /// Allocator-assigned slices.
    pub used_slices: u64,
    /// Available slices.
    pub free_slices: u64,
}

#[doc(hidden)]
#[derive(Clone, Copy, Debug)]
pub struct RegionFields {
    pub region_index: u32,
    pub reserved_bytes: u64,
    pub used_slices: u64,
    pub free_slices: u64,
}

impl Region {
    #[doc(hidden)]
    #[must_use]
    pub fn from_fields(fields: RegionFields) -> Self {
        let RegionFields {
            region_index,
            reserved_bytes,
            used_slices,
            free_slices,
        } = fields;
        Self {
            region_index,
            reserved_bytes,
            used_slices,
            free_slices,
        }
    }
}

/// Allocation-domain aggregate state.
#[non_exhaustive]
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Domain {
    /// Process-local domain identifier.
    pub domain_id: u64,
    /// Whether this is the default domain.
    pub is_default: bool,
    /// Number of owned regions.
    pub region_count: u64,
    /// Reserved virtual bytes.
    pub reserved_bytes: u64,
    /// Allocator-assigned slices.
    pub used_slices: u64,
    /// Available slices.
    pub free_slices: u64,
    /// Slices used for small slabs.
    pub small_slices: u64,
    /// Slices used for medium spans.
    pub medium_slices: u64,
    /// Slices used for bump heaps.
    pub bump_slices: u64,
    /// Assigned slices without a stable classification.
    pub unknown_slices: u64,
    /// Indices of owned regions.
    pub region_indices: Vec<u32>,
}

#[doc(hidden)]
#[derive(Debug)]
pub struct DomainFields {
    pub domain_id: u64,
    pub is_default: bool,
    pub region_count: u64,
    pub reserved_bytes: u64,
    pub used_slices: u64,
    pub free_slices: u64,
    pub small_slices: u64,
    pub medium_slices: u64,
    pub bump_slices: u64,
    pub unknown_slices: u64,
    pub region_indices: Vec<u32>,
}

impl Domain {
    #[doc(hidden)]
    #[must_use]
    pub fn from_fields(fields: DomainFields) -> Self {
        let DomainFields {
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
        } = fields;
        Self {
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
        }
    }
}

/// Allocation-size histograms.
#[non_exhaustive]
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Histograms {
    /// Counts of allocated sizes by bucket.
    pub allocated: Vec<u64>,
    /// Counts of live sizes by bucket.
    pub live: Vec<u64>,
}

#[doc(hidden)]
#[derive(Debug)]
pub struct HistogramsFields {
    pub allocated: Vec<u64>,
    pub live: Vec<u64>,
}

impl Histograms {
    #[doc(hidden)]
    #[must_use]
    pub fn from_fields(fields: HistogramsFields) -> Self {
        let HistogramsFields { allocated, live } = fields;
        Self { allocated, live }
    }
}

/// Metadata associated with a snapshot.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Metadata {
    /// Version of the enclosing wire format.
    pub wire_format_version: u16,
    /// Version of the decoded telemetry schema.
    pub telemetry_schema_version: u16,
    /// Version of the snapshot producer.
    pub producer_version: Version,
    /// Time spent collecting source state and resolving in-capture symbol information.
    ///
    /// Symbol-cache prewarming, final sizing/encoding, destruction and file I/O
    /// are excluded. This is a
    /// partial collection duration, not end-to-end snapshot latency. A containing
    /// process snapshot can supply its own collection duration to a report.
    pub capture_duration_nanos: u64,
}

/// A snapshot section skipped because its identifier is unknown or its version is older or newer
/// than the versions supported by this decoder.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SkippedSection {
    /// Wire section identifier.
    pub id: u16,
    /// Version reported by the skipped section.
    pub version: u16,
}

#[doc(hidden)]
#[derive(Clone, Copy, Debug)]
pub struct SkippedSectionFields {
    pub id: u16,
    pub version: u16,
}

impl SkippedSection {
    #[doc(hidden)]
    #[must_use]
    pub fn from_fields(fields: SkippedSectionFields) -> Self {
        let SkippedSectionFields { id, version } = fields;
        Self { id, version }
    }
}

/// Complete owned allocator telemetry snapshot.
#[non_exhaustive]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Snapshot {
    /// Snapshot metadata.
    pub metadata: Metadata,
    /// Process-wide allocator statistics.
    pub stats: Stats,
    /// Per-size-class statistics.
    pub size_classes: Vec<SizeClass>,
    /// Aggregate region statistics.
    pub regions: Vec<Region>,
    /// Physical allocator topology.
    pub topology: Vec<crate::topology::TopologyRegion>,
    /// Allocation-domain statistics.
    pub domains: Vec<Domain>,
    /// Retained caller data, when captured.
    pub callers: Option<crate::callers::Callers>,
    /// Retained ownership and synchronization events, when captured.
    pub runtime_events: Option<seismograph::recorder::event::Events>,
    /// Allocation-size histograms.
    pub histograms: Histograms,
    /// Symbol information for caller addresses.
    pub addresses: Vec<crate::callers::AddressLookup>,
    /// Sections skipped because their identifiers were unknown or their versions were unsupported.
    pub skipped_sections: Vec<SkippedSection>,
}

impl Snapshot {
    /// Creates an empty snapshot for `producer_version`.
    #[must_use]
    pub fn new(producer_version: Version) -> Self {
        Self {
            metadata: Metadata {
                wire_format_version: crate::wire::format::Header::new(1, producer_version).wire_format(),
                telemetry_schema_version: crate::TELEMETRY_SCHEMA_VERSION,
                producer_version,
                capture_duration_nanos: 0,
            },
            stats: Stats::default(),
            size_classes: Vec::new(),
            regions: Vec::new(),
            topology: Vec::new(),
            domains: Vec::new(),
            callers: None,
            runtime_events: None,
            histograms: Histograms::default(),
            addresses: Vec::new(),
            skipped_sections: Vec::new(),
        }
    }
}
