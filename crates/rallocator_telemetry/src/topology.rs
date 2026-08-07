// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Physical allocator topology model types.

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

impl Segment {
    #[doc(hidden)]
    #[must_use]
    pub const fn new(
        segment_index: u8,
        class_index: u32,
        context: bool,
        live_blocks: u32,
        usable_blocks: u32,
        utilization_tracked: bool,
    ) -> Self {
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

/// A physical allocator slice.
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

impl Slice {
    #[doc(hidden)]
    #[must_use]
    pub const fn new(
        slice_index: u32,
        kind: SliceKind,
        span_slices: u32,
        owner: u64,
        requested_bytes: u64,
        usable_bytes: u64,
        segments: Vec<Segment>,
    ) -> Self {
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

impl TopologyRegion {
    #[doc(hidden)]
    #[must_use]
    pub const fn new(
        region_index: u32,
        base_address: u64,
        region_bytes: u64,
        slice_bytes: u64,
        used_bitmap: Vec<u64>,
        slices: Vec<Slice>,
    ) -> Self {
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
