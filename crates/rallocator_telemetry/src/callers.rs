// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Retained caller and symbol model types.

/// Per-thread retained event-log summary.
#[non_exhaustive]
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ThreadLog {
    /// Thread-log identifier.
    pub thread_log_id: u64,
    /// Events observed by this log.
    pub total_events: u64,
    /// Events overwritten before capture.
    pub lost_events: u64,
    /// Allocated-size counts by bucket.
    pub allocated_histogram: Vec<u64>,
    /// Live-size counts by bucket.
    pub live_histogram: Vec<u64>,
}

impl ThreadLog {
    #[doc(hidden)]
    #[must_use]
    pub const fn new(
        thread_log_id: u64,
        total_events: u64,
        lost_events: u64,
        allocated_histogram: Vec<u64>,
        live_histogram: Vec<u64>,
    ) -> Self {
        Self {
            thread_log_id,
            total_events,
            lost_events,
            allocated_histogram,
            live_histogram,
        }
    }
}

/// Kind of recorded allocation event.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum EventKind {
    /// Allocation event.
    #[default]
    Allocated,
    /// Deallocation event.
    Deallocated,
}

/// Kind of heap that recorded an event.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum HeapKind {
    /// General-purpose heap.
    #[default]
    General,
    /// Bump heap.
    Bump,
    /// Thread-local heap.
    Thread,
}

/// A retained allocation or deallocation event.
#[non_exhaustive]
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Event {
    /// Owning thread-log identifier.
    pub thread_log_id: u64,
    /// Thread that recorded this event.
    pub event_thread_id: u64,
    /// Sequence number within the log.
    pub sequence: u64,
    /// Stable allocation identifier.
    pub allocation_id: u64,
    /// Allocation or deallocation kind.
    pub kind: EventKind,
    /// Heap identifier.
    pub heap_id: u64,
    /// Heap classification.
    pub heap_kind: HeapKind,
    /// Whether a bump heap was released before this free.
    pub freed_after_heap_release: bool,
    /// Allocation address.
    pub address: u64,
    /// Allocation size in bytes.
    pub size: u64,
    /// Allocation alignment in bytes.
    pub align: u64,
    /// Captured instruction-pointer frames.
    pub call_stack: Vec<u64>,
}

impl Event {
    #[doc(hidden)]
    #[must_use]
    #[expect(
        clippy::too_many_arguments,
        reason = "The full constructor preserves schema completeness for producers"
    )]
    pub const fn new(
        thread_log_id: u64,
        event_thread_id: u64,
        sequence: u64,
        allocation_id: u64,
        kind: EventKind,
        heap_id: u64,
        heap_kind: HeapKind,
        freed_after_heap_release: bool,
        address: u64,
        size: u64,
        align: u64,
        call_stack: Vec<u64>,
    ) -> Self {
        Self {
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
        }
    }
}

/// Name associated with a recorded thread.
#[non_exhaustive]
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ThreadName {
    /// Thread identifier.
    pub thread_id: u64,
    /// Captured thread name.
    pub name: String,
}

impl ThreadName {
    #[doc(hidden)]
    #[must_use]
    pub const fn new(thread_id: u64, name: String) -> Self {
        Self { thread_id, name }
    }
}

/// Retained caller telemetry.
#[non_exhaustive]
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Callers {
    /// Capture-session identifier.
    pub session_id: u64,
    /// Events observed during the session.
    pub total_events: u64,
    /// Events lost before capture.
    pub lost_events: u64,
    /// Per-thread log summaries.
    pub threads: Vec<ThreadLog>,
    /// Retained allocation events.
    pub events: Vec<Event>,
    /// Captured thread names.
    pub thread_names: Vec<ThreadName>,
}

impl Callers {
    #[doc(hidden)]
    #[must_use]
    pub const fn new(
        session_id: u64,
        total_events: u64,
        lost_events: u64,
        threads: Vec<ThreadLog>,
        events: Vec<Event>,
        thread_names: Vec<ThreadName>,
    ) -> Self {
        Self {
            session_id,
            total_events,
            lost_events,
            threads,
            events,
            thread_names,
        }
    }
}

/// Symbol lookup information for one instruction address.
#[non_exhaustive]
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AddressLookup {
    /// Instruction address.
    pub address: u64,
    /// Resolved symbol name.
    pub symbol: Option<String>,
    /// Resolved source filename.
    pub filename: Option<String>,
    /// Resolved source line.
    pub line: Option<u32>,
    /// Resolved source column.
    pub column: Option<u32>,
}

impl AddressLookup {
    #[doc(hidden)]
    #[must_use]
    pub const fn new(address: u64, symbol: Option<String>, filename: Option<String>, line: Option<u32>, column: Option<u32>) -> Self {
        Self {
            address,
            symbol,
            filename,
            line,
            column,
        }
    }
}
