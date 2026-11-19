// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

//! Backend-aware allocator observations for Seismograph.
//!
//! [`snapshot::Snapshot`] combines a shared observation header with typed
//! [`snmalloc`] or [`mimalloc`] state. Its schema-5 codec reads both backends.
//! Other schemas are rejected. Allocation events remain independent of
//! backend state in the surrounding Seismograph container.
//!
//! Mimalloc state includes bounded page summaries from owner threads and
//! independently sampled arena, abandoned-page, and direct-OS state. Missing,
//! busy, failed, and exited owners remain explicit. Used blocks can include
//! uncollected cross-thread frees. Committed bytes are not physical residency.
//! Direct-OS commitment covers data only, not metadata.
//!
//! The snmalloc model, [`native::Snapshot`], inventories persistent owners, including never-observed
//! active endpoints, and an independently collected global backend. Owner state
//! is bounded and can be stale: compare lease generation, round, and capture time.
//! Idle inspection is fresh under the pool lock; busy, unobserved and slot
//! allocation failure (`Unavailable`) are explicitly unknown. Native capacity is not application-live
//! memory, remote batching budget is not pending bytes, and globally cached
//! ranges are not guaranteed physically decommitted.
//! Matching rounds mean contributed this round, not an exact current census:
//! the first round can predate polling. Consumers show observation age at capture.
//! Outstanding allocator ranges include pending/retained frees, not app-live
//! object counts. Incoming front/back inequality means potential work only,
//! not guaranteed ready links or queue depth; equality does not prove emptiness.
//!
//! [`encoded_len`], [`encode`] and [`decode`] handle snmalloc in the current schema.
//! Use [`snapshot`] to encode or decode either backend.
//! The decoder bounds allocation by both the payload length and [`MAX_OWNERS`],
//! rejects invalid flags, duplicates, inconsistent inventory and trailing bytes.
//! Application allocation, resize, and free events remain in the Seismograph container.
//! [`encoded_len_with_owners`] and [`encode_with_owners`] accept borrowed owner
//! rows, ignoring the metadata snapshot's vector. Both use fixed stack scratch
//! and never allocate, so a producer can keep inventory and output System-backed
//! without native allocator activity perturbing the observations it collects.
//! [`events::callers`] projects these into view-local correlation identities,
//! preserving stacks, actor names, orphan records and repeated addresses.
//! Successful resizes keep the original view-local lifetime across address changes.
//! Allocation counts describe original allocations. Unmatched size summaries use
//! the last retained resize. Missing events can still hide lifetime endpoints.
//!
//! Self-publication defaults on, but recording defaults off. Polling requests
//! another observation round without a background thread or per-operation clock
//! check. Controls belong to this plugin, not the allocator's public API.
//! Operations with an enabled allocation policy publish after the native operation
//! ends, at most once per lease and requested round. Event sampling does not
//! control publication. Collectors request the next round after collection.
//! Applications can also call [`native::request_observation`] explicitly.
//!
//! # Explore a sample capture
//!
//! ```text
//! cargo +1.95.0 run -p seismograph_rallocator --example native_snapshot
//! cargo +1.95.0 run -p seismograph_cli -- view native-demo.seismograph
//! ```
//!
//! The example uses synthetic native state and real recorder events, including
//! address reuse and an orphan free. It does not install the native allocator.
//! Its capture callback encodes borrowed stack rows into System-backed `SourceData`.

pub mod callers;
mod codec;
pub mod events;
pub mod mimalloc;
pub mod native;
mod observation;
pub mod snapshot;
pub mod snmalloc;

pub use codec::{Error, ErrorKind, MAX_OWNERS, decode, encode, encode_with_owners, encoded_len, encoded_len_with_owners};

/// Stable identity and schema metadata for the allocator source.
pub mod source {
    /// Stable Seismograph source identity.
    pub const ID: seismograph::snapshot::SourceId = seismograph::snapshot::SourceId::new(0x5241_4c4c_4f43_4154);
    /// Human-readable source name.
    pub const NAME: &str = "rallocator";
    /// Current allocator payload schema for either backend.
    pub const SCHEMA_VERSION: u16 = crate::snapshot::SCHEMA_VERSION;
    /// Stable identity of the backend-specific state source for mimalloc.
    pub const MIMALLOC_ID: seismograph::snapshot::SourceId = seismograph::snapshot::SourceId::new(0x4d49_4d41_4c4c_4f43);
    /// Display name of the state source for mimalloc.
    pub const MIMALLOC_NAME: &str = "rallocator/mimalloc";
}
