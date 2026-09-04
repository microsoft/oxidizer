// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Rallocator virtual-address topology model types.

/// Classifies an allocator slice.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SliceKind {
    /// Slice without a stable classification.
    #[default]
    Unknown,
    /// Slice containing small-allocation slabs.
    Small,
    /// First slice of a medium allocation span.
    Medium,
    /// Subsequent slice of a medium allocation span.
    MediumContinuation,
    /// Slice used by a bump heap.
    Bump,
}

/// A small-allocation slab segment.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Segment {
    /// Segment index within its slice.
    pub segment_index: u8,
    /// Allocator size-class index.
    pub class_index: u32,
    /// Whether this is a context segment.
    pub context: bool,
    /// Number of live blocks.
    pub live_blocks: u32,
    /// Number of usable blocks.
    pub usable_blocks: u32,
    /// Whether utilization counters were captured.
    pub utilization_tracked: bool,
}

#[doc(hidden)]
#[derive(Clone, Copy, Debug)]
pub struct SegmentFields {
    pub segment_index: u8,
    pub class_index: u32,
    pub context: bool,
    pub live_blocks: u32,
    pub usable_blocks: u32,
    pub utilization_tracked: bool,
}

impl Segment {
    #[doc(hidden)]
    #[must_use]
    pub fn from_fields(fields: SegmentFields) -> Self {
        let SegmentFields {
            segment_index,
            class_index,
            context,
            live_blocks,
            usable_blocks,
            utilization_tracked,
        } = fields;
        Self {
            segment_index,
            class_index,
            context,
            live_blocks,
            usable_blocks,
            utilization_tracked,
        }
    }
}

/// A slice of an allocator-managed virtual-address region.
#[non_exhaustive]
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Slice {
    /// Slice index within its region.
    pub slice_index: u32,
    /// Slice classification.
    pub kind: SliceKind,
    /// Span length for the first slice of a span.
    pub span_slices: u32,
    /// Owning heap or allocation identifier.
    pub owner: u64,
    /// Requested bytes for a medium span.
    pub requested_bytes: u64,
    /// Usable bytes for a medium span.
    pub usable_bytes: u64,
    /// Small-allocation slab segments.
    pub segments: Vec<Segment>,
}

#[doc(hidden)]
#[derive(Debug)]
pub struct SliceFields {
    pub slice_index: u32,
    pub kind: SliceKind,
    pub span_slices: u32,
    pub owner: u64,
    pub requested_bytes: u64,
    pub usable_bytes: u64,
    pub segments: Vec<Segment>,
}

impl Slice {
    #[doc(hidden)]
    #[must_use]
    pub fn from_fields(fields: SliceFields) -> Self {
        let SliceFields {
            slice_index,
            kind,
            span_slices,
            owner,
            requested_bytes,
            usable_bytes,
            segments,
        } = fields;
        Self {
            slice_index,
            kind,
            span_slices,
            owner,
            requested_bytes,
            usable_bytes,
            segments,
        }
    }
}

/// Topology detail for an allocator region.
#[non_exhaustive]
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TopologyRegion {
    /// Region index.
    pub region_index: u32,
    /// Region base address.
    pub base_address: u64,
    /// Region size in bytes.
    pub region_bytes: u64,
    /// Slice size in bytes.
    pub slice_bytes: u64,
    /// Bitmap of assigned slices.
    pub used_bitmap: Vec<u64>,
    /// Detailed assigned slices.
    pub slices: Vec<Slice>,
}

#[doc(hidden)]
#[derive(Debug)]
pub struct TopologyRegionFields {
    pub region_index: u32,
    pub base_address: u64,
    pub region_bytes: u64,
    pub slice_bytes: u64,
    pub used_bitmap: Vec<u64>,
    pub slices: Vec<Slice>,
}

impl TopologyRegion {
    #[doc(hidden)]
    #[must_use]
    pub fn from_fields(fields: TopologyRegionFields) -> Self {
        let TopologyRegionFields {
            region_index,
            base_address,
            region_bytes,
            slice_bytes,
            used_bitmap,
            slices,
        } = fields;
        Self {
            region_index,
            base_address,
            region_bytes,
            slice_bytes,
            used_bitmap,
            slices,
        }
    }
}
