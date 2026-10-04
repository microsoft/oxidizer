// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Private presentation regression fixtures, never decoded from native sources.

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum SliceKind {
    #[default]
    Unknown,
    Small,
    Medium,
    Bump,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct Segment {
    pub(crate) index: u8,
    pub(crate) class_index: u32,
    pub(crate) context: bool,
    pub(crate) live_blocks: u32,
    pub(crate) usable_blocks: u32,
    pub(crate) utilization_tracked: bool,
}
#[cfg(test)]
pub(crate) type SegmentFields = Segment;
#[cfg(test)]
impl Segment {
    pub(crate) const fn from_fields(fields: Self) -> Self {
        fields
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct Slice {
    pub(crate) index: u32,
    pub(crate) kind: SliceKind,
    pub(crate) span_slices: u32,
    pub(crate) owner: u64,
    pub(crate) requested_bytes: u64,
    pub(crate) usable_bytes: u64,
    pub(crate) segments: Vec<Segment>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct TopologyRegion {
    pub(crate) region_index: u32,
    pub(crate) base_address: u64,
    pub(crate) region_bytes: u64,
    pub(crate) slice_bytes: u64,
    pub(crate) used_bitmap: Vec<u64>,
    pub(crate) slices: Vec<Slice>,
}
