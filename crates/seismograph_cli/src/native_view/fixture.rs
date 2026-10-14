// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Shared native explorer fixture: every coverage category and partial walks.

use seismograph_rallocator::native::{ClassState, Observation, ObservationSource, Owner, Snapshot};

fn published_observation() -> Observation {
    let mut observation = Observation {
        generation: 3,
        session_id: 7,
        round: 5,
        thread_id: 42,
        captured_nanos: 90,
        ..Observation::EMPTY
    };
    observation.classes[1] = ClassState {
        object_bytes: 32,
        slab_bytes: 65536,
        capacity: 2046,
        available_slabs: 2,
        empty_slabs: 1,
        observed_slabs: 3,
        fast_nonempty: true,
    };
    observation.classes[43] = ClassState {
        object_bytes: 32768,
        slab_bytes: 524_288,
        capacity: 15,
        available_slabs: 1,
        empty_slabs: 0,
        observed_slabs: 1,
        fast_nonempty: false,
    };
    observation.large.counts[22] = 2;
    observation.large.complete = false;
    observation.local.ranges.counts[20] = 3;
    observation.local.metadata.counts[16] = 1;
    observation.local.requested_bytes = 123_456;
    observation.remote.open_rings = 1;
    observation.remote.open_objects = 2;
    observation.remote.outgoing_lists = 1;
    observation.remote.messages = 2;
    observation.remote.message_objects = 8;
    observation.remote.message_bytes = 256;
    observation.remote.budget_remaining = 999;
    observation.remote.incoming_front = 0xabc;
    observation.remote.incoming_back = 0xdef;
    observation.remote.complete = false;
    observation
}

pub(crate) fn snapshot() -> Snapshot {
    let observation = published_observation();
    let owner = Owner {
        id: 0x100,
        generation: 3,
        leased: true,
        source: ObservationSource::Published,
        observation: Some(observation),
    };
    let mut snapshot = Snapshot {
        captured_nanos: 100,
        session_id: 7,
        round: 5,
        owner_count: 9,
        owners_complete: false,
        publication_enabled: true,
        owners: vec![
            owner,
            Owner {
                id: 0x200,
                source: ObservationSource::Unobserved,
                observation: None,
                ..owner
            },
            Owner {
                id: 0x300,
                generation: 5,
                ..owner
            },
            Owner {
                id: 0x400,
                observation: Some(Observation {
                    session_id: 6,
                    ..observation
                }),
                ..owner
            },
            Owner {
                id: 0x500,
                generation: 4,
                leased: false,
                source: ObservationSource::IdleInspection,
                observation: Some(Observation {
                    generation: 4,
                    session_id: 0,
                    thread_id: 0,
                    ..observation
                }),
            },
            Owner {
                id: 0x600,
                source: ObservationSource::Busy,
                observation: None,
                ..owner
            },
            Owner {
                id: 0x700,
                source: ObservationSource::Busy,
                observation: Some(Observation { round: 4, ..observation }),
                ..owner
            },
        ],
        ..Snapshot::default()
    };
    snapshot.global.reserved_bytes = 12 << 20;
    snapshot.global.pagemap_reserved_bytes = 16 << 30;
    snapshot.global.local_limit_bytes = 32 << 20;
    snapshot.global.global_refill_bytes = 1 << 30;
    snapshot.global.ranges.counts[25] = 3;
    snapshot.global.ranges.complete = false;
    snapshot
}
