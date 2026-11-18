// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Backend-aware allocator snapshots. The decoder accepts only the current schema.
//!
//! Header fields describe the observation boundary, not a simultaneous census.
//! Payloads keep backend-specific meanings. Allocation events remain unchanged
//! in the surrounding Seismograph container.

mod codec;

pub use codec::{decode, encode, encode_mimalloc, encode_snmalloc, encoded_len, encoded_len_mimalloc, encoded_len_snmalloc};

pub use crate::observation::{captured_nanos, observation_round, publication_enabled, request_observation, set_publication_enabled};

/// Current backend-aware payload schema.
pub const SCHEMA_VERSION: u16 = 5;

/// Common collection boundary and owner coverage.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Header {
    /// Process-relative monotonic capture time.
    pub captured_nanos: u64,
    /// Producer-defined recorder session, or zero for recording-independent observations.
    pub session_id: u64,
    /// Requested observation round.
    pub round: u64,
    /// Total known owner lifetimes, including omitted rows.
    pub owner_count: u64,
    /// Whether all owner rows appear in this capture.
    pub owners_complete: bool,
    /// Whether operations with allocation recording enabled can publish owner copies.
    pub publication_enabled: bool,
}

/// Allocator-specific state with no cross-backend field reinterpretation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BackendState {
    /// Snmalloc slabs, return messages, and range caches.
    Snmalloc(crate::snmalloc::State),
    /// Mimalloc pages, abandoned indexes, and arena bitmaps.
    Mimalloc(crate::mimalloc::State),
}

/// Shared observation header and one backend-specific payload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Snapshot {
    /// Collection boundary and inventory coverage.
    pub header: Header,
    /// State whose interpretation is determined by the backend.
    pub backend: BackendState,
}

impl From<crate::native::Snapshot> for Snapshot {
    fn from(snapshot: crate::native::Snapshot) -> Self {
        Self {
            header: Header {
                captured_nanos: snapshot.captured_nanos,
                session_id: snapshot.session_id,
                round: snapshot.round,
                owner_count: snapshot.owner_count,
                owners_complete: snapshot.owners_complete,
                publication_enabled: snapshot.publication_enabled,
            },
            backend: BackendState::Snmalloc(crate::snmalloc::State {
                owners: snapshot.owners,
                global: snapshot.global,
            }),
        }
    }
}

impl From<(Header, crate::snmalloc::State)> for crate::native::Snapshot {
    fn from((header, state): (Header, crate::snmalloc::State)) -> Self {
        Self {
            captured_nanos: header.captured_nanos,
            session_id: header.session_id,
            round: header.round,
            owner_count: header.owner_count,
            owners_complete: header.owners_complete,
            publication_enabled: header.publication_enabled,
            owners: state.owners,
            global: state.global,
        }
    }
}
