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

#[doc(hidden)]
#[derive(Debug)]
pub struct ThreadLogFields {
    pub thread_log_id: u64,
    pub total_events: u64,
    pub lost_events: u64,
    pub allocated_histogram: Vec<u64>,
    pub live_histogram: Vec<u64>,
}

impl ThreadLog {
    #[doc(hidden)]
    #[must_use]
    pub fn from_fields(fields: ThreadLogFields) -> Self {
        let ThreadLogFields {
            thread_log_id,
            total_events,
            lost_events,
            allocated_histogram,
            live_histogram,
        } = fields;
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
#[non_exhaustive]
pub enum EventKind {
    /// Allocation event.
    #[default]
    Allocated,
    /// Deallocation event.
    Deallocated,
}

/// Kind of heap that recorded an event.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
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

#[doc(hidden)]
#[derive(Debug)]
pub struct EventFields {
    pub thread_log_id: u64,
    pub event_thread_id: u64,
    pub sequence: u64,
    pub allocation_id: u64,
    pub kind: EventKind,
    pub heap_id: u64,
    pub heap_kind: HeapKind,
    pub freed_after_heap_release: bool,
    pub address: u64,
    pub size: u64,
    pub align: u64,
    pub call_stack: Vec<u64>,
}

impl Event {
    #[doc(hidden)]
    #[must_use]
    pub fn from_fields(fields: EventFields) -> Self {
        let EventFields {
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
        } = fields;
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

#[doc(hidden)]
#[derive(Debug)]
pub struct ThreadNameFields {
    pub thread_id: u64,
    pub name: String,
}

impl ThreadName {
    #[doc(hidden)]
    #[must_use]
    pub fn from_fields(fields: ThreadNameFields) -> Self {
        let ThreadNameFields { thread_id, name } = fields;
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

#[doc(hidden)]
#[derive(Debug)]
pub struct CallersFields {
    pub session_id: u64,
    pub total_events: u64,
    pub lost_events: u64,
    pub threads: Vec<ThreadLog>,
    pub events: Vec<Event>,
    pub thread_names: Vec<ThreadName>,
}

impl Callers {
    #[doc(hidden)]
    #[must_use]
    pub fn from_fields(fields: CallersFields) -> Self {
        let CallersFields {
            session_id,
            total_events,
            lost_events,
            threads,
            events,
            thread_names,
        } = fields;
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

#[doc(hidden)]
#[derive(Debug)]
pub struct AddressLookupFields {
    pub address: u64,
    pub symbol: Option<String>,
    pub filename: Option<String>,
    pub line: Option<u32>,
    pub column: Option<u32>,
}

impl AddressLookup {
    #[doc(hidden)]
    #[must_use]
    pub fn from_fields(fields: AddressLookupFields) -> Self {
        let AddressLookupFields {
            address,
            symbol,
            filename,
            line,
            column,
        } = fields;
        Self {
            address,
            symbol,
            filename,
            line,
            column,
        }
    }
}
