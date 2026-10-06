// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Native codec bounds, provenance and publication-control regressions.

#![expect(clippy::unwrap_used, reason = "Fixture construction must fail immediately")]

use seismograph_rallocator::native::{
    ClassState, Freshness, GlobalState, LocalState, Observation, ObservationSource, Owner, Ranges, RemoteState, Snapshot,
};
use seismograph_rallocator::{ErrorKind, decode, encode, encode_with_owners, encoded_len, encoded_len_with_owners};

#[test]
fn empty_observation_defaults_and_freshness_labels_preserve_unknown_state() {
    assert_eq!(Observation::default(), Observation::EMPTY);
    for (freshness, label) in [
        (Freshness::Unknown, "unknown / unobserved"),
        (Freshness::IdleInspection, "fresh idle inspection"),
        (Freshness::Current, "contributed this round"),
        (Freshness::PreviousLease, "stale lease"),
        (Freshness::PreviousSession, "stale session"),
        (Freshness::PreviousRound, "older round"),
        (Freshness::NewerThanCapture, "newer than capture"),
    ] {
        assert_eq!(freshness.label(), label);
    }
}

fn fixture() -> Snapshot {
    let ranges = Ranges {
        counts: core::array::from_fn(|index| index as u64 + 1),
        complete: false,
    };
    let observation = Observation {
        generation: 9,
        session_id: 10,
        round: 11,
        thread_id: 12,
        captured_nanos: 13,
        slabs_complete: false,
        classes: core::array::from_fn(|index| ClassState {
            object_bytes: index as u64 + 14,
            slab_bytes: 15,
            capacity: 16,
            available_slabs: 17,
            empty_slabs: 18,
            observed_slabs: 19,
            fast_nonempty: index % 2 == 0,
        }),
        large: ranges,
        local: LocalState {
            ranges,
            metadata: Ranges::EMPTY,
            requested_bytes: 20,
        },
        remote: RemoteState {
            open_rings: 21,
            open_objects: 22,
            outgoing_lists: 23,
            messages: 24,
            message_objects: 25,
            message_bytes: 26,
            complete: false,
            budget_remaining: 27,
            incoming_front: 28,
            incoming_back: 29,
        },
    };
    Snapshot {
        captured_nanos: 30,
        session_id: 10,
        round: 11,
        owner_count: 5,
        owners_complete: false,
        publication_enabled: true,
        owners: vec![
            Owner {
                id: 1,
                generation: 9,
                leased: true,
                source: ObservationSource::Published,
                observation: Some(observation),
            },
            Owner {
                id: 2,
                generation: 9,
                leased: true,
                source: ObservationSource::Unobserved,
                observation: None,
            },
            Owner {
                id: 3,
                generation: 10,
                leased: false,
                source: ObservationSource::IdleInspection,
                observation: Some(Observation {
                    generation: 10,
                    session_id: 0,
                    thread_id: 0,
                    ..observation
                }),
            },
            Owner {
                id: 4,
                generation: 11,
                leased: true,
                source: ObservationSource::Busy,
                observation: Some(observation),
            },
        ],
        global: GlobalState {
            reserved_bytes: 31,
            ranges,
            pagemap_reserved_bytes: 32,
            local_limit_bytes: 33,
            global_refill_bytes: 34,
        },
    }
}

fn bytes(snapshot: &Snapshot) -> Vec<u8> {
    let mut bytes = vec![0; encoded_len(snapshot).unwrap()];
    assert_eq!(encode(snapshot, &mut bytes).unwrap(), bytes.len());
    bytes
}

#[test]
fn all_native_fields_round_trip() {
    let snapshot = fixture();
    assert_eq!(decode(&bytes(&snapshot)).unwrap(), snapshot);
}

#[test]
fn borrowed_rows_override_metadata_vector_and_match_owned_encoding() {
    let snapshot = fixture();
    let expected = bytes(&snapshot);
    let metadata = Snapshot {
        owners: vec![Owner::default()],
        ..snapshot
    };
    let mut output = vec![0; encoded_len_with_owners(&metadata, &snapshot.owners).unwrap()];
    encode_with_owners(&metadata, &snapshot.owners, &mut output).unwrap();
    assert_eq!(output, expected);
}

#[test]
fn borrowed_rows_validate_inventory_counts_and_exact_output_lengths() {
    let mut metadata = fixture();
    let owners = std::mem::take(&mut metadata.owners);
    let length = encoded_len_with_owners(&metadata, &owners).unwrap();
    let mut output = vec![0xa5; length - 1];
    assert_eq!(
        encode_with_owners(&metadata, &owners, &mut output).unwrap_err().kind(),
        ErrorKind::LengthMismatch
    );
    assert!(output.iter().all(|byte| *byte == 0xa5));
    metadata.owners_complete = true;
    assert_eq!(
        encoded_len_with_owners(&metadata, &owners).unwrap_err().kind(),
        ErrorKind::Malformed
    );
    metadata.owners_complete = false;
    metadata.owner_count = 0;
    assert_eq!(
        encoded_len_with_owners(&metadata, &owners).unwrap_err().kind(),
        ErrorKind::Malformed
    );
}

#[test]
fn borrowed_duplicate_detection_covers_chunk_boundaries() {
    let metadata = Snapshot {
        owner_count: 1025,
        owners_complete: true,
        ..Snapshot::default()
    };
    let mut owners = (1..=1025).map(|id| Owner { id, ..Owner::default() }).collect::<Vec<_>>();
    let mut output = vec![0; encoded_len_with_owners(&metadata, &owners).unwrap()];
    encode_with_owners(&metadata, &owners, &mut output).unwrap();
    assert_eq!(decode(&output).unwrap().owners, owners);
    owners[1024].id = owners[0].id;
    assert_eq!(
        encoded_len_with_owners(&metadata, &owners).unwrap_err().kind(),
        ErrorKind::DuplicateOwner
    );
}

#[test]
fn zero_source_round_trips_without_inventing_coverage() {
    let snapshot = Snapshot::default();
    assert_eq!(decode(&bytes(&snapshot)).unwrap(), snapshot);
}

#[test]
fn every_truncation_is_rejected() {
    let bytes = bytes(&fixture());
    for length in 0..bytes.len() {
        assert!(decode(&bytes[..length]).is_err(), "prefix {length}");
    }
}

#[test]
fn trailing_input_and_wrong_output_lengths_are_rejected() {
    let snapshot = fixture();
    let mut bytes = bytes(&snapshot);
    bytes.push(0);
    assert_eq!(decode(&bytes).unwrap_err().kind(), ErrorKind::LengthMismatch);
    assert_eq!(encode(&snapshot, &mut bytes).unwrap_err().kind(), ErrorKind::LengthMismatch);
    assert_eq!(encode(&snapshot, &mut []).unwrap_err().kind(), ErrorKind::LengthMismatch);
}

#[test]
fn forged_inventory_count_cannot_allocate_unbounded_memory() {
    let mut bytes = bytes(&Snapshot::default());
    bytes[46..50].copy_from_slice(&u32::MAX.to_le_bytes());
    assert_eq!(decode(&bytes).unwrap_err().kind(), ErrorKind::LengthOverflow);
}

#[test]
fn strict_flags_schema_reserved_and_source_tags() {
    let original = bytes(&fixture());
    for (offset, value, expected) in [
        (0, 0, ErrorKind::InvalidMagic),
        (8, 1, ErrorKind::UnsupportedSchema(1)),
        (10, 1, ErrorKind::Malformed),
        (44, 2, ErrorKind::Malformed),
        (45, 2, ErrorKind::Malformed),
        (595 + 16, 2, ErrorKind::Malformed),
        (595 + 17, 5, ErrorKind::Malformed),
        (595 + 18, 2, ErrorKind::Malformed),
        (595 + 19 + 40, 2, ErrorKind::Malformed),
        (595 + 19 + 41 + 48, 2, ErrorKind::Malformed),
    ] {
        let mut malformed = original.clone();
        malformed[offset] = value;
        assert_eq!(decode(&malformed).unwrap_err().kind(), expected, "offset {offset}");
    }
}

#[test]
fn duplicate_owners_and_inconsistent_inventory_are_rejected() {
    let mut snapshot = fixture();
    snapshot.owners.push(snapshot.owners[0]);
    assert_eq!(encoded_len(&snapshot).unwrap_err().kind(), ErrorKind::DuplicateOwner);
    let mut snapshot = fixture();
    snapshot.owners_complete = true;
    assert_eq!(encoded_len(&snapshot).unwrap_err().kind(), ErrorKind::Malformed);
}

#[test]
fn malformed_duplicate_wire_owner_is_rejected() {
    let snapshot = Snapshot {
        owner_count: 2,
        owners_complete: true,
        owners: vec![Owner { id: 1, ..Owner::default() }, Owner { id: 2, ..Owner::default() }],
        ..Snapshot::default()
    };
    let mut bytes = bytes(&snapshot);
    bytes[595 + 19..595 + 27].copy_from_slice(&1_u64.to_le_bytes());
    assert_eq!(decode(&bytes).unwrap_err().kind(), ErrorKind::DuplicateOwner);
}

#[test]
fn unknown_source_cannot_carry_fabricated_zero_observation() {
    let mut snapshot = fixture();
    snapshot.owners[1].observation = Some(Observation::EMPTY);
    assert_eq!(encoded_len(&snapshot).unwrap_err().kind(), ErrorKind::Malformed);
}

#[test]
fn unavailable_slot_round_trips_as_explicit_failure_not_unobserved() {
    let snapshot = Snapshot {
        owner_count: 1,
        owners_complete: true,
        owners: vec![Owner {
            id: 42,
            generation: 1,
            leased: true,
            source: ObservationSource::Unavailable,
            observation: None,
        }],
        ..Snapshot::default()
    };
    assert_eq!(decode(&bytes(&snapshot)).unwrap(), snapshot);
}

#[test]
fn unavailable_slot_rejects_fabricated_observation_in_memory_and_on_wire() {
    let mut snapshot = fixture();
    snapshot.owners[0].source = ObservationSource::Unavailable;
    assert_eq!(encoded_len(&snapshot).unwrap_err().kind(), ErrorKind::Malformed);
    let mut malformed = bytes(&fixture());
    malformed[595 + 17] = 4;
    assert_eq!(decode(&malformed).unwrap_err().kind(), ErrorKind::Malformed);
}

#[test]
fn current_idle_unknown_and_old_lease_are_distinct() {
    let snapshot = fixture();
    assert_eq!(
        snapshot.owners.iter().map(|owner| owner.freshness(&snapshot)).collect::<Vec<_>>(),
        vec![
            Freshness::Current,
            Freshness::Unknown,
            Freshness::IdleInspection,
            Freshness::PreviousLease
        ]
    );
}

#[test]
fn stale_session_round_and_future_are_distinct() {
    let snapshot = fixture();
    let owner = snapshot.owners[0];
    let mut observation = owner.observation.unwrap();
    observation.session_id = 9;
    assert_eq!(
        Owner {
            observation: Some(observation),
            ..owner
        }
        .freshness(&snapshot),
        Freshness::PreviousSession
    );
    observation.session_id = 10;
    observation.round = 10;
    assert_eq!(
        Owner {
            observation: Some(observation),
            ..owner
        }
        .freshness(&snapshot),
        Freshness::PreviousRound
    );
    observation.round = 12;
    assert_eq!(
        Owner {
            observation: Some(observation),
            ..owner
        }
        .freshness(&snapshot),
        Freshness::NewerThanCapture
    );
}

#[test]
fn native_capacity_sum_does_not_overflow_u64() {
    let ranges = Ranges {
        counts: [u64::MAX; 64],
        complete: false,
    };
    assert_eq!(ranges.observed_bytes(), u128::from(u64::MAX) * u128::from(u64::MAX));
}

#[test]
fn controls_advance_without_background_work() {
    let round = seismograph_rallocator::native::observation_round();
    assert!(seismograph_rallocator::native::request_observation() >= round);
    let before = seismograph_rallocator::native::captured_nanos();
    assert!(seismograph_rallocator::native::captured_nanos() >= before);
    assert!(seismograph_rallocator::native::publication_enabled());
    seismograph_rallocator::native::set_publication_enabled(false);
    assert!(!seismograph_rallocator::native::publication_enabled());
    seismograph_rallocator::native::set_publication_enabled(true);
}
