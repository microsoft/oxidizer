// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! I/O event types.

use std::num::NonZeroU64;
use std::sync::atomic::{AtomicU64, Ordering};

macro_rules! identifier {
    ($name:ident, $description:literal) => {
        #[doc = $description]
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        #[repr(transparent)]
        pub struct $name(NonZeroU64);

        impl $name {
            /// Reconstructs an identifier from its stable numeric representation.
            #[must_use]
            pub const fn from_raw(value: u64) -> Option<Self> {
                match NonZeroU64::new(value) {
                    Some(value) => Some(Self(value)),
                    None => None,
                }
            }

            /// Returns the stable numeric representation.
            #[must_use]
            pub const fn get(self) -> u64 {
                self.0.get()
            }
        }
    };
}

identifier!(BufferId, "Process-monotonic identity assigned to a logical byte buffer.");
identifier!(IoOperationId, "Process-monotonic identity assigned to one recorded I/O operation.");
identifier!(IoResourceId, "Process-monotonic identity assigned to one recorded I/O resource.");

static NEXT_BUFFER_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_IO_OPERATION_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_IO_RESOURCE_ID: AtomicU64 = AtomicU64::new(1);

fn allocate_id(counter: &AtomicU64) -> NonZeroU64 {
    let value = counter
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| current.checked_add(1))
        .expect("a process cannot create u64::MAX telemetry identities");
    NonZeroU64::new(value).expect("generated identity counters start at one")
}

impl BufferId {
    /// Allocates a process-monotonic buffer identity.
    #[must_use]
    pub fn allocate() -> Self {
        Self(allocate_id(&NEXT_BUFFER_ID))
    }
}

impl IoOperationId {
    /// Allocates a process-monotonic I/O operation identity.
    #[must_use]
    pub fn allocate() -> Self {
        Self(allocate_id(&NEXT_IO_OPERATION_ID))
    }
}

impl IoResourceId {
    /// Allocates a process-monotonic I/O resource identity.
    #[must_use]
    pub fn allocate() -> Self {
        Self(allocate_id(&NEXT_IO_RESOURCE_ID))
    }
}

/// Kind of primitive that performed an I/O operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum IoResourceKind {
    /// File-like storage.
    File,
    /// Connected TCP byte stream.
    TcpStream,
    /// TCP listener accepting connections.
    TcpListener,
    /// Named pipe.
    NamedPipe,
    /// WinHTTP request handle.
    WinHttpRequest,
    /// Resource without a more specific classification.
    Other,
}

impl IoResourceKind {
    pub(crate) const fn wire_value(self) -> u8 {
        match self {
            Self::File => 1,
            Self::TcpStream => 2,
            Self::TcpListener => 3,
            Self::NamedPipe => 4,
            Self::WinHttpRequest => 5,
            Self::Other => 6,
        }
    }

    pub(crate) const fn from_wire_value(value: u8) -> Option<Self> {
        match value {
            1 => Some(Self::File),
            2 => Some(Self::TcpStream),
            3 => Some(Self::TcpListener),
            4 => Some(Self::NamedPipe),
            5 => Some(Self::WinHttpRequest),
            6 => Some(Self::Other),
            _ => None,
        }
    }
}

/// Outcome of a completed I/O operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum IoOutcome {
    /// Operation has started but has not completed.
    Pending,
    /// Operation completed successfully.
    Success,
    /// A read reached the end of its input stream.
    EndOfStream,
    /// Operation was canceled.
    Canceled,
    /// Operation failed.
    Error,
}

impl IoOutcome {
    pub(crate) const fn wire_value(self) -> u8 {
        match self {
            Self::Pending => 1,
            Self::Success => 2,
            Self::EndOfStream => 3,
            Self::Canceled => 4,
            Self::Error => 5,
        }
    }

    pub(crate) const fn from_wire_value(value: u8) -> Option<Self> {
        match value {
            1 => Some(Self::Pending),
            2 => Some(Self::Success),
            3 => Some(Self::EndOfStream),
            4 => Some(Self::Canceled),
            5 => Some(Self::Error),
            _ => None,
        }
    }
}

/// Fixed numeric context carried by an I/O event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IoEvent {
    /// Identity shared by the start and finish events of one operation.
    pub operation_id: IoOperationId,
    /// Primitive that performed the operation.
    pub resource_id: IoResourceId,
    /// Logical byte buffer or view involved in the operation, when known.
    pub buffer_id: Option<BufferId>,
    /// Number of bytes requested by the caller.
    pub requested_bytes: u64,
    /// Number of bytes transferred when the operation completed.
    pub completed_bytes: u64,
    /// Logical buffer length observed at this event.
    pub buffer_len: u64,
    /// Number of spans in the logical buffer observed at this event.
    pub buffer_span_count: u32,
    /// Kind of primitive that performed the operation.
    pub resource_kind: IoResourceKind,
    /// Current operation outcome.
    pub outcome: IoOutcome,
}
