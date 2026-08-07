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

impl Estimate {
    #[doc(hidden)]
    #[must_use]
    pub const fn new(value: u64, lower_bound: u64, upper_bound: u64) -> Self {
        Self {
            value,
            lower_bound,
            upper_bound,
        }
    }
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

impl Stats {
    #[doc(hidden)]
    #[must_use]
    #[expect(
        clippy::too_many_arguments,
        reason = "The full constructor preserves schema completeness for producers"
    )]
    pub const fn new(
        allocated_bytes: u64,
        deallocated_bytes: u64,
        live_bytes: u64,
        peak_live_bytes: u64,
        mapped_bytes: u64,
        os_mappings: u64,
        os_unmappings: u64,
        allocations: u64,
        deallocations: u64,
        remote_frees: u64,
        pending_remote_blocks: u64,
        remote_pushes_in_progress: u64,
        drained_remote_blocks: u64,
    ) -> Self {
        Self {
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

impl SizeClass {
    #[doc(hidden)]
    #[must_use]
    pub const fn new(
        class_index: u32,
        block_bytes: u64,
        live_allocations: Estimate,
        requested_bytes: Estimate,
        usable_bytes: Estimate,
    ) -> Self {
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

impl Region {
    #[doc(hidden)]
    #[must_use]
    pub const fn new(region_index: u32, reserved_bytes: u64, used_slices: u64, free_slices: u64) -> Self {
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

impl Domain {
    #[doc(hidden)]
    #[must_use]
    #[expect(
        clippy::too_many_arguments,
        reason = "The full constructor preserves schema completeness for producers"
    )]
    pub const fn new(
        domain_id: u64,
        is_default: bool,
        region_count: u64,
        reserved_bytes: u64,
        used_slices: u64,
        free_slices: u64,
        small_slices: u64,
        medium_slices: u64,
        bump_slices: u64,
        unknown_slices: u64,
        region_indices: Vec<u32>,
    ) -> Self {
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

impl Histograms {
    #[doc(hidden)]
    #[must_use]
    pub const fn new(allocated: Vec<u64>, live: Vec<u64>) -> Self {
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
    /// Time spent collecting the snapshot.
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

impl SkippedSection {
    #[doc(hidden)]
    #[must_use]
    pub const fn new(id: u16, version: u16) -> Self {
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
