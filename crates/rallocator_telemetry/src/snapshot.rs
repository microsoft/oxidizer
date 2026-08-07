// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Core snapshot model types.

/// Producer version carried by snapshot metadata.
pub use rallocator_wire::format::Version;

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

/// Process-wide allocator counters.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Stats {
    /// Cumulative requested bytes allocated.
    pub allocated_bytes: u64,
    /// Cumulative requested bytes deallocated.
    pub deallocated_bytes: u64,
    /// Currently live requested bytes.
    pub live_bytes: u64,
    /// Highest observed live requested bytes.
    pub peak_live_bytes: u64,
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
    /// Cross-thread deallocation operations.
    pub remote_frees: u64,
    /// Remote blocks awaiting reclamation.
    pub pending_remote_blocks: u64,
    /// Remote push operations in progress.
    pub remote_pushes_in_progress: u64,
    /// Remote blocks reclaimed by owners.
    pub drained_remote_blocks: u64,
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

/// Allocation-size histograms.
#[non_exhaustive]
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Histograms {
    /// Counts of allocated sizes by bucket.
    pub allocated: Vec<u64>,
    /// Counts of live sizes by bucket.
    pub live: Vec<u64>,
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
    /// Time spent collecting the snapshot.
    pub capture_duration_nanos: u64,
}

/// A recognized snapshot section skipped because its version is newer than this decoder supports.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SkippedSection {
    /// Wire section identifier.
    pub id: u16,
    /// Unsupported section version.
    pub version: u16,
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
    /// Allocation-size histograms.
    pub histograms: Histograms,
    /// Symbol information for caller addresses.
    pub addresses: Vec<crate::callers::AddressLookup>,
    /// Recognized sections skipped because their versions were unsupported.
    pub skipped_sections: Vec<SkippedSection>,
}

impl Snapshot {
    /// Creates an empty snapshot for `producer_version`.
    #[must_use]
    pub fn new(producer_version: Version) -> Self {
        Self {
            metadata: Metadata {
                wire_format_version: rallocator_wire::format::Header::new(1, producer_version).wire_format(),
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
            histograms: Histograms::default(),
            addresses: Vec::new(),
            skipped_sections: Vec::new(),
        }
    }
}
