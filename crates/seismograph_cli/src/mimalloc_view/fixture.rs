// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Deterministic backend snapshots in real Seismograph containers.

use seismograph::snapshot::SourceSnapshot;
use seismograph_rallocator::mimalloc::{ClassState, GlobalState, Observation, ObservationSource, Owner, State};
use seismograph_rallocator::snapshot::{self, BackendState, Header, Snapshot};

fn observation() -> Observation {
    let mut observation = Observation {
        session_id: 7,
        round: 5,
        thread_id: 42,
        captured_nanos: 90,
        pages_complete: false,
        free_lists_complete: false,
        ..Observation::EMPTY
    };
    observation.classes[1] = ClassState {
        block_bytes: 32,
        page_bytes: 65_536,
        queued_pages: 4,
        observed_pages: 2,
        reserved_blocks: 128,
        capacity_blocks: 100,
        used_blocks: 40,
        free_blocks: 20,
        local_free_blocks: 10,
        retired_pages: 1,
    };
    observation.classes[73] = ClassState {
        queued_pages: 1,
        observed_pages: 1,
        reserved_blocks: 4,
        capacity_blocks: 2,
        used_blocks: 1,
        free_blocks: 1,
        ..ClassState::default()
    };
    observation
}

pub(crate) fn snapshot() -> Snapshot {
    let observation = observation();
    let owner = Owner {
        id: 11,
        active: true,
        source: ObservationSource::Published,
        observation: Some(observation),
    };
    let mut global = GlobalState {
        arena_count: 2,
        reserved_bytes: 8192,
        committed_bytes: 4096,
        free_slices: 3,
        pending_purge_slices: 2,
        metadata_bytes: 512,
        os_allocations: 1,
        os_reserved_bytes: 2048,
        os_committed_bytes: 1024,
        arenas_complete: false,
        ..GlobalState::EMPTY
    };
    global.abandoned_pages[1] = 3;
    global.abandoned_pages[60] = 2;
    Snapshot {
        header: Header {
            captured_nanos: 100,
            session_id: 7,
            round: 5,
            owner_count: 12,
            owners_complete: false,
            publication_enabled: true,
        },
        backend: BackendState::Mimalloc(State {
            global,
            owners: vec![
                owner,
                Owner {
                    id: 12,
                    source: ObservationSource::Unobserved,
                    observation: None,
                    ..owner
                },
                Owner {
                    id: 13,
                    source: ObservationSource::Unavailable,
                    observation: None,
                    ..owner
                },
                Owner {
                    id: 14,
                    source: ObservationSource::Busy,
                    observation: None,
                    ..owner
                },
                Owner {
                    id: 15,
                    source: ObservationSource::Busy,
                    observation: Some(Observation { round: 4, ..observation }),
                    ..owner
                },
                Owner {
                    id: 16,
                    active: false,
                    ..owner
                },
                Owner {
                    id: 17,
                    observation: Some(Observation {
                        session_id: 6,
                        ..observation
                    }),
                    ..owner
                },
                Owner {
                    id: 18,
                    observation: Some(Observation {
                        captured_nanos: 101,
                        ..observation
                    }),
                    ..owner
                },
                Owner {
                    id: 19,
                    observation: Some(Observation { round: 4, ..observation }),
                    ..owner
                },
                Owner {
                    id: 20,
                    active: false,
                    source: ObservationSource::Unobserved,
                    observation: None,
                },
            ],
        }),
    }
}

pub(crate) fn source(snapshot: &Snapshot) -> SourceSnapshot {
    let (id, name) = match &snapshot.backend {
        BackendState::Snmalloc(_) => (seismograph_rallocator::source::ID, seismograph_rallocator::source::NAME),
        BackendState::Mimalloc(_) => (
            seismograph_rallocator::source::MIMALLOC_ID,
            seismograph_rallocator::source::MIMALLOC_NAME,
        ),
    };
    let mut data = vec![0; snapshot::encoded_len(snapshot).unwrap()];
    snapshot::encode(snapshot, &mut data).unwrap();
    SourceSnapshot {
        id,
        name: name.into(),
        schema_version: snapshot::SCHEMA_VERSION,
        data,
    }
}

pub(crate) fn container(sources: &[SourceSnapshot]) -> Vec<u8> {
    // Current container: one recorder thread and one ArcClone with a captured stack.
    let mut bytes = b"SEISMOG\0".to_vec();
    bytes.extend_from_slice(&10_u16.to_le_bytes());
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    bytes.extend_from_slice(&0_u64.to_le_bytes());
    bytes.extend_from_slice(&1_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u32.to_le_bytes());
    bytes.extend_from_slice(&u32::try_from(sources.len()).unwrap().to_le_bytes());
    for _ in 0..6 {
        bytes.extend_from_slice(&[0, 0, 0, 0, 1, 0, 0, 0]);
    }
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    bytes.extend_from_slice(&1_000_000_000_u64.to_le_bytes());
    bytes.extend_from_slice(&41_u64.to_le_bytes());
    bytes.extend_from_slice(&1_u64.to_le_bytes());
    bytes.extend_from_slice(&0_u64.to_le_bytes());
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    bytes.extend_from_slice(&41_u64.to_le_bytes());
    bytes.extend_from_slice(&1_u64.to_le_bytes());
    bytes.extend_from_slice(&1_u64.to_le_bytes());
    bytes.extend_from_slice(&[seismograph::recorder::event::EventKind::ArcClone.wire_value(), 1, 1, 0]);
    bytes.extend_from_slice(&7_u64.to_le_bytes());
    bytes.extend_from_slice(&[0; 7 * 8]);
    bytes.extend_from_slice(&0x1234_u64.to_le_bytes());
    for source in sources {
        bytes.extend_from_slice(&source.id.get().to_le_bytes());
        bytes.extend_from_slice(&source.schema_version.to_le_bytes());
        bytes.extend_from_slice(&u16::try_from(source.name.len()).unwrap().to_le_bytes());
        bytes.extend_from_slice(&u64::try_from(source.data.len()).unwrap().to_le_bytes());
        bytes.extend_from_slice(&0_u32.to_le_bytes());
        bytes.extend_from_slice(source.name.as_bytes());
        bytes.extend_from_slice(&source.data);
    }
    bytes.extend_from_slice(&0_u32.to_le_bytes());
    bytes
}
