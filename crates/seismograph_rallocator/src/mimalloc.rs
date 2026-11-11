// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Bounded mimalloc observations. Capacity and used blocks are not application liveness.
//!
//! Queued pages exclude pages already transferred to the abandoned index.
//! A used block can await collection after a cross-thread free.
//! Global atomics are read independently. Committed bytes describe allocator
//! tracking, not physical residency. Observation never collects or purges memory.

/// Number of page bins, including the singleton bin and unused bin positions.
pub const CLASS_COUNT: usize = 74;
/// Regular bin positions covered by the abandoned-page index.
pub const ABANDONED_CLASS_COUNT: usize = 61;

/// Owner-local page state for one bin.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ClassState {
    /// Rounded block size, or zero when this bin has no fixed block size.
    pub block_bytes: u64,
    /// Ordinary backing page size, or zero for variable-sized singletons.
    pub page_bytes: u64,
    /// Pages in the owner's queue, including pages omitted by the walk.
    pub queued_pages: u64,
    /// Pages visited by the bounded observation.
    pub observed_pages: u64,
    /// Block slots in the visited pages, including slots not yet initialized.
    pub reserved_blocks: u64,
    /// Initialized block slots in the visited pages.
    pub capacity_blocks: u64,
    /// Owner-accounted used blocks, including uncollected cross-thread frees.
    pub used_blocks: u64,
    /// Links visited in allocation-ready free lists.
    pub free_blocks: u64,
    /// Links visited in locally deferred free lists.
    pub local_free_blocks: u64,
    /// Visited pages with an active retirement countdown.
    pub retired_pages: u64,
}

/// A copy made by the owning thread after an operation with allocation recording enabled.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Observation {
    /// Producer-defined recorder session, or zero for recording-independent observations.
    pub session_id: u64,
    /// Requested observation round.
    pub round: u64,
    /// Contributing recorder thread.
    pub thread_id: u64,
    /// Process-relative monotonic capture time.
    pub captured_nanos: u64,
    /// False when the page visit budget ended.
    pub pages_complete: bool,
    /// False when the free-list visit budget ended.
    pub free_lists_complete: bool,
    /// State of each owner-local page bin.
    pub classes: [ClassState; CLASS_COUNT],
}

impl Observation {
    /// Empty, complete state to populate under exclusive owner access.
    pub const EMPTY: Self = Self {
        session_id: 0,
        round: 0,
        thread_id: 0,
        captured_nanos: 0,
        pages_complete: true,
        free_lists_complete: true,
        classes: [ClassState {
            block_bytes: 0,
            page_bytes: 0,
            queued_pages: 0,
            observed_pages: 0,
            reserved_blocks: 0,
            capacity_blocks: 0,
            used_blocks: 0,
            free_blocks: 0,
            local_free_blocks: 0,
            retired_pages: 0,
        }; CLASS_COUNT],
    };
}

impl Default for Observation {
    fn default() -> Self {
        Self::EMPTY
    }
}

/// Origin or availability of a published observation.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ObservationSource {
    /// The owner never published readable state.
    #[default]
    Unobserved,
    /// The owner supplied a synchronized copy.
    Published,
    /// The publication slot was busy during this capture.
    Busy,
    /// Allocation of diagnostic storage failed.
    Unavailable,
}

/// One heap lifetime. Heap identifiers are never reused.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Owner {
    /// Stable allocator heap identifier, not a pointer or a recorder thread ID.
    pub id: u64,
    /// False after heap cleanup ends.
    pub active: bool,
    /// Availability of the diagnostic slot.
    pub source: ObservationSource,
    /// Last readable copy. Exited owners retain historical state only.
    pub observation: Option<Observation>,
}

impl Owner {
    /// Classifies the copy without treating missing or exited state as current.
    #[must_use]
    pub fn freshness(&self, header: &crate::snapshot::Header) -> crate::native::Freshness {
        use crate::native::Freshness;
        let Some(observation) = &self.observation else {
            return Freshness::Unknown;
        };
        if !self.active {
            return Freshness::PreviousLease;
        }
        if observation.session_id != header.session_id {
            return Freshness::PreviousSession;
        }
        if observation.round > header.round || observation.captured_nanos > header.captured_nanos {
            return Freshness::NewerThanCapture;
        }
        if observation.round < header.round {
            return Freshness::PreviousRound;
        }
        Freshness::Current
    }
}

/// Independently sampled arena and direct-OS state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GlobalState {
    /// Published arenas visited by the collector.
    pub arena_count: u64,
    /// Total virtual reservation of those arenas.
    pub reserved_bytes: u64,
    /// Allocator-tracked committed arena bytes, including committed metadata.
    pub committed_bytes: u64,
    /// Arena slices currently advertised as free.
    pub free_slices: u64,
    /// Slice bits that await a purge attempt. Some slices cannot be purged.
    pub pending_purge_slices: u64,
    /// Permanently reserved arena metadata bytes.
    pub metadata_bytes: u64,
    /// Live direct-OS backing reservations.
    pub os_allocations: u64,
    /// Virtual bytes reserved by live direct-OS allocations, including metadata.
    pub os_reserved_bytes: u64,
    /// Committed direct-OS data bytes. Metadata commitment is not included.
    pub os_committed_bytes: u64,
    /// Published regular abandoned-index counts, not live object counts.
    ///
    /// Only bins below [`ABANDONED_CLASS_COUNT`] are indexed. Other entries
    /// are zero placeholders, not evidence that no abandoned pages exist.
    pub abandoned_pages: [u64; CLASS_COUNT],
    /// False when a bounded arena scan omitted published arenas.
    pub arenas_complete: bool,
}

impl GlobalState {
    /// Empty state before the allocator creates any backing.
    pub const EMPTY: Self = Self {
        arena_count: 0,
        reserved_bytes: 0,
        committed_bytes: 0,
        free_slices: 0,
        pending_purge_slices: 0,
        metadata_bytes: 0,
        os_allocations: 0,
        os_reserved_bytes: 0,
        os_committed_bytes: 0,
        abandoned_pages: [0; CLASS_COUNT],
        arenas_complete: true,
    };
}

impl Default for GlobalState {
    fn default() -> Self {
        Self::EMPTY
    }
}

/// Mimalloc owner copies and independently sampled global state.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct State {
    /// Registered heap lifetimes, including exited and unknown owners.
    pub owners: Vec<Owner>,
    /// Approximate global memory state.
    pub global: GlobalState,
}
