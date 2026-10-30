// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Writes a synthetic native-v4 inventory beside real recorded allocation events.
//! No allocator implementation is replaced or emulated by this example.
//! Its source encodes borrowed stack inventory into System-backed `SourceData`.
//!
//! `cargo +1.95.0 run -p seismograph_rallocator --example native_snapshot`
//! `cargo +1.95.0 run -p seismograph_cli -- view native-demo.seismograph`
//! `cargo +1.95.0 run -p seismograph_cli -- snapshot html native-demo.seismograph`

use seismograph::recorder::alloc::{Allocation, AllocationId, EventThreadId, HeapId, HeapKind};
use seismograph::recorder::event::{Address, EventClass, Record};
use seismograph_rallocator::native::{self, ClassState, Observation, ObservationSource, Owner, Snapshot};

static SOURCE: seismograph::snapshot::Source = seismograph::snapshot::Source::new(
    seismograph_rallocator::source::ID,
    "synthetic native v4 demo",
    seismograph_rallocator::source::SCHEMA_VERSION,
    capture,
);

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output = std::env::args_os()
        .nth(1)
        .map_or_else(|| std::path::PathBuf::from("native-demo.seismograph"), std::path::PathBuf::from);
    seismograph::snapshot::register_source(&SOURCE);
    seismograph::recorder(seismograph::recorder::Configuration {
        allocations: seismograph::recorder::RecordingPolicy {
            enabled: true,
            capture_backtraces: true,
            ..Default::default()
        },
        ..Default::default()
    });
    operation(0x4000, false);
    std::thread::Builder::new()
        .name("demo-returner".into())
        .spawn(|| {
            operation(0x4000, true);
            operation(0x9000, true);
        })?
        .join()
        .expect("the demo worker has no fallible operations");
    operation(0x4000, false);
    operation(0x4000, true);
    operation(0x4000, false);
    seismograph::snapshot(seismograph::snapshot::SnapshotOptions::default())?.write_file(&output)?;
    seismograph::recorder(seismograph::recorder::Configuration::default());
    println!("{}", output.display());
    Ok(())
}

fn operation(key: u64, free: bool) {
    seismograph::record(EventClass::Allocation, || {
        let allocation = Allocation {
            allocation_id: AllocationId::new(key),
            event_thread_id: EventThreadId::new(0),
            heap_id: HeapId::new(0),
            heap_kind: HeapKind::General,
            freed_after_heap_release: false,
            address: Address::new(key),
            size: 32,
            alignment: 8,
        };
        Some(if free {
            Record::deallocation(allocation)
        } else {
            Record::allocation(allocation)
        })
    });
}

#[expect(
    clippy::large_stack_arrays,
    reason = "Six fixed demo rows avoid invoking the application allocator during capture"
)]
fn capture(context: seismograph::snapshot::SnapshotContext<'_>) -> Result<seismograph::snapshot::SourceData, seismograph::Error> {
    let session_id = context.recording_observation().map_or(0, |observation| observation.session.get());
    let round = native::observation_round();
    let mut observation = Observation {
        generation: 1,
        session_id,
        round,
        thread_id: context.events().threads.first().map_or(0, |thread| thread.thread_id.get()),
        captured_nanos: native::captured_nanos(),
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
    observation.large.counts[22] = 2;
    observation.large.complete = false;
    observation.local.ranges.counts[20] = 3;
    observation.local.metadata.counts[16] = 1;
    observation.remote.open_rings = 1;
    observation.remote.open_objects = 2;
    observation.remote.message_bytes = 256;
    observation.remote.budget_remaining = 999;
    observation.remote.incoming_back = 0xabc;
    let owner = Owner {
        id: 0x100,
        generation: 1,
        leased: true,
        source: ObservationSource::Published,
        observation: Some(observation),
    };
    let owners = [
        owner,
        Owner {
            id: 0x200,
            source: ObservationSource::Unobserved,
            observation: None,
            ..owner
        },
        Owner {
            id: 0x300,
            generation: 3,
            ..owner
        },
        Owner {
            id: 0x400,
            generation: 2,
            leased: false,
            source: ObservationSource::IdleInspection,
            observation: Some(Observation {
                generation: 2,
                thread_id: 0,
                session_id: 0,
                ..observation
            }),
        },
        Owner {
            id: 0x500,
            source: ObservationSource::Busy,
            observation: None,
            ..owner
        },
        Owner {
            id: 0x600,
            source: ObservationSource::Unavailable,
            observation: None,
            ..owner
        },
    ];
    let mut snapshot = Snapshot {
        captured_nanos: native::captured_nanos(),
        session_id,
        round,
        owner_count: owners.len() as u64,
        owners_complete: true,
        publication_enabled: native::publication_enabled(),
        ..Snapshot::default()
    };
    snapshot.global.ranges.counts[25] = 3;
    snapshot.global.reserved_bytes = 12 << 20;
    snapshot.global.pagemap_reserved_bytes = 16 << 30;
    let mut data = seismograph::snapshot::SourceData::zeroed(
        seismograph_rallocator::encoded_len_with_owners(&snapshot, &owners).expect("the fixed demo inventory obeys the schema bounds"),
    )?;
    seismograph_rallocator::encode_with_owners(&snapshot, &owners, data.as_mut_bytes())
        .expect("SourceData has the validated exact encoded length");
    native::request_observation();
    Ok(data)
}
