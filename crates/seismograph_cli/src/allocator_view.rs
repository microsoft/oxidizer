// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Private event-view projection and presentation regression fixtures.
//! No source codec or compatibility API is attached to this model.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Version {
    pub(crate) major: u16,
    pub(crate) minor: u16,
    pub(crate) patch: u16,
}
impl Version {
    pub(crate) const fn new(major: u16, minor: u16, patch: u16) -> Self {
        Self { major, minor, patch }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct Estimate {
    pub(crate) value: u64,
    pub(crate) lower_bound: u64,
    pub(crate) upper_bound: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum PeakLiveBytesScope {
    #[default]
    Unavailable,
}
impl std::fmt::Display for PeakLiveBytesScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Unavailable => "unavailable",
        })
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct Stats {
    pub(crate) allocated_bytes: u64,
    pub(crate) deallocated_bytes: u64,
    pub(crate) live_bytes: u64,
    pub(crate) peak_live_bytes: u64,
    pub(crate) peak_live_bytes_scope: PeakLiveBytesScope,
    pub(crate) mapped_bytes: u64,
    pub(crate) os_mappings: u64,
    pub(crate) os_unmappings: u64,
    pub(crate) allocations: u64,
    pub(crate) deallocations: u64,
    pub(crate) remote_frees: u64,
    pub(crate) pending_remote_blocks: u64,
    pub(crate) remote_pushes_in_progress: u64,
    pub(crate) drained_remote_blocks: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct SizeClass {
    pub(crate) class_index: u32,
    pub(crate) block_bytes: u64,
    pub(crate) live_allocations: Estimate,
    pub(crate) requested_bytes: Estimate,
    pub(crate) usable_bytes: Estimate,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct Region {
    pub(crate) index: u32,
    pub(crate) reserved_bytes: u64,
    pub(crate) used_slices: u64,
    pub(crate) free_slices: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct Domain {
    pub(crate) id: u64,
    pub(crate) is_default: bool,
    pub(crate) region_count: u64,
    pub(crate) reserved_bytes: u64,
    pub(crate) used_slices: u64,
    pub(crate) free_slices: u64,
    pub(crate) small_slices: u64,
    pub(crate) medium_slices: u64,
    pub(crate) bump_slices: u64,
    pub(crate) unknown_slices: u64,
    pub(crate) region_indices: Vec<u32>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct Histograms {
    pub(crate) allocated: Vec<u64>,
    pub(crate) live: Vec<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Metadata {
    pub(crate) wire_format_version: u16,
    pub(crate) telemetry_schema_version: u16,
    pub(crate) producer_version: Version,
    pub(crate) capture_duration_nanos: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SkippedSection {
    pub(crate) id: u16,
    pub(crate) version: u16,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Snapshot {
    pub(crate) native: Option<std::sync::Arc<seismograph_rallocator::native::Snapshot>>,
    pub(crate) metadata: Metadata,
    pub(crate) allocator_state_available: bool,
    pub(crate) stats: Stats,
    pub(crate) size_classes: Vec<SizeClass>,
    pub(crate) regions: Vec<Region>,
    pub(crate) topology: Vec<crate::allocator_topology::TopologyRegion>,
    pub(crate) domains: Vec<Domain>,
    pub(crate) callers: Option<seismograph_rallocator::callers::Callers>,
    pub(crate) runtime_events: Option<seismograph::recorder::event::Events>,
    pub(crate) histograms: Histograms,
    pub(crate) addresses: Vec<seismograph_rallocator::callers::AddressLookup>,
    pub(crate) skipped_sections: Vec<SkippedSection>,
}
impl Snapshot {
    pub(crate) fn new(producer_version: Version) -> Self {
        Self {
            native: None,
            metadata: Metadata {
                wire_format_version: 1,
                telemetry_schema_version: 3,
                producer_version,
                capture_duration_nanos: 0,
            },
            allocator_state_available: true,
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
    pub(crate) fn event_only(producer_version: Version) -> Self {
        let mut snapshot = Self::new(producer_version);
        snapshot.allocator_state_available = false;
        snapshot
    }
}
