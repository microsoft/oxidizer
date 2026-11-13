// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Backend-aware wire compatibility, validation, and observation semantics.

use seismograph_rallocator::mimalloc::{ClassState, GlobalState, Observation, ObservationSource, Owner, State};
use seismograph_rallocator::snapshot::{self, BackendState, Header, Snapshot};
use seismograph_rallocator::{ErrorKind, native};

fn fixture() -> Snapshot {
    let mut observation = Observation {
        session_id: 7,
        round: 9,
        thread_id: 11,
        captured_nanos: 13,
        ..Observation::default()
    };
    observation.classes[8] = ClassState {
        block_bytes: 64,
        page_bytes: 65536,
        queued_pages: 2,
        observed_pages: 1,
        reserved_blocks: 1016,
        capacity_blocks: 128,
        used_blocks: 17,
        free_blocks: 90,
        local_free_blocks: 21,
        retired_pages: 0,
    };
    observation.pages_complete = false;
    let mut global = GlobalState {
        arena_count: 2,
        reserved_bytes: 1 << 30,
        committed_bytes: 1 << 20,
        free_slices: 100,
        pending_purge_slices: 20,
        metadata_bytes: 1 << 16,
        os_allocations: 3,
        os_reserved_bytes: 1 << 24,
        os_committed_bytes: 1 << 23,
        arenas_complete: false,
        ..GlobalState::default()
    };
    global.abandoned_pages[60] = 37;
    Snapshot {
        header: Header {
            captured_nanos: 19,
            session_id: 7,
            round: 9,
            owner_count: 5,
            owners_complete: true,
            publication_enabled: true,
        },
        backend: BackendState::Mimalloc(State {
            owners: vec![
                Owner {
                    id: 16,
                    active: true,
                    source: ObservationSource::Published,
                    observation: Some(observation),
                },
                Owner {
                    id: 32,
                    active: false,
                    source: ObservationSource::Published,
                    observation: Some(observation),
                },
                Owner {
                    id: 48,
                    active: true,
                    source: ObservationSource::Busy,
                    observation: None,
                },
                Owner {
                    id: 64,
                    active: true,
                    source: ObservationSource::Unavailable,
                    observation: None,
                },
                Owner {
                    id: 80,
                    active: true,
                    source: ObservationSource::Unobserved,
                    observation: None,
                },
            ],
            global,
        }),
    }
}

#[expect(clippy::unwrap_used, reason = "The fixture must produce a valid payload")]
fn encoded(snapshot: &Snapshot) -> Vec<u8> {
    let mut output = vec![0; snapshot::encoded_len(snapshot).unwrap()];
    snapshot::encode(snapshot, &mut output).unwrap();
    output
}

#[test]
fn both_backends_round_trip_without_field_reinterpretation() {
    let mimalloc = fixture();
    let snmalloc = Snapshot::from(native::Snapshot {
        captured_nanos: 29,
        session_id: 31,
        round: 37,
        owner_count: 1,
        owners_complete: true,
        publication_enabled: false,
        owners: vec![native::Owner {
            id: 41,
            ..native::Owner::default()
        }],
        global: native::GlobalState {
            reserved_bytes: 43,
            ..native::GlobalState::default()
        },
    });

    let actual = [&mimalloc, &snmalloc].map(|state| snapshot::decode(&encoded(state)).unwrap());

    assert_eq!(actual, [mimalloc, snmalloc]);
}

#[test]
fn snmalloc_encoder_uses_the_current_backend_aware_schema() {
    let native = native::Snapshot {
        captured_nanos: 17,
        session_id: 23,
        owner_count: 1,
        owners_complete: true,
        owners: vec![native::Owner {
            id: 64,
            ..Default::default()
        }],
        ..Default::default()
    };
    let mut input = vec![0; seismograph_rallocator::encoded_len(&native).unwrap()];
    seismograph_rallocator::encode(&native, &mut input).unwrap();

    let decoded = snapshot::decode(&input).unwrap();
    let BackendState::Snmalloc(state) = decoded.backend else {
        panic!("the snmalloc encoder must identify its backend")
    };
    let reconstructed = native::Snapshot::from((decoded.header, state));

    assert_eq!(reconstructed, native);
    for version in 0..snapshot::SCHEMA_VERSION {
        input[8..10].copy_from_slice(&version.to_le_bytes());
        assert_eq!(
            snapshot::decode(&input).unwrap_err().kind(),
            seismograph_rallocator::ErrorKind::UnsupportedSchema(version)
        );
        assert_eq!(
            seismograph_rallocator::decode(&input).unwrap_err().kind(),
            seismograph_rallocator::ErrorKind::UnsupportedSchema(version)
        );
    }
}

#[test]
fn mimalloc_borrowed_encoder_matches_the_owned_encoder_and_checks_output_length() {
    let value = fixture();
    let BackendState::Mimalloc(state) = &value.backend else {
        unreachable!()
    };
    let length = snapshot::encoded_len_mimalloc(&value.header, &state.global, &state.owners).unwrap();
    let mut output = vec![0; length];

    snapshot::encode_mimalloc(&value.header, &state.global, &state.owners, &mut output).unwrap();
    let truncated = snapshot::encode_mimalloc(&value.header, &state.global, &state.owners, &mut output[..length - 1]);

    assert_eq!(
        (output, truncated.unwrap_err().kind()),
        (encoded(&value), ErrorKind::LengthMismatch)
    );
}

#[test]
fn forged_headers_tags_and_counts_are_rejected() {
    let valid = encoded(&fixture());
    let cases = [
        (0, 0, ErrorKind::InvalidMagic),
        (8, 99, ErrorKind::UnsupportedSchema(99)),
        (10, 99, ErrorKind::UnsupportedBackend(99)),
        (11, 1, ErrorKind::Malformed),
        (44, 2, ErrorKind::Malformed),
        (45, 2, ErrorKind::Malformed),
        (49, 255, ErrorKind::LengthOverflow),
    ];

    for (offset, value, expected) in cases {
        let mut input = valid.clone();
        input[offset] = value;
        assert_eq!(snapshot::decode(&input).unwrap_err().kind(), expected, "offset {offset}");
    }
}

#[test]
fn truncated_and_trailing_payloads_never_decode_as_complete() {
    let input = encoded(&fixture());
    let prefixes = if cfg!(miri) {
        vec![0, 7, 11, 45, 49, 714, input.len() - 1]
    } else {
        (0..input.len()).collect()
    };
    for length in prefixes {
        assert!(snapshot::decode(&input[..length]).is_err(), "accepted prefix {length}");
    }
    let mut trailing = input;
    trailing.push(0);
    assert_eq!(snapshot::decode(&trailing).unwrap_err().kind(), ErrorKind::LengthMismatch);
}

#[test]
fn invalid_owner_inventory_and_class_accounting_are_rejected() {
    let mut value = fixture();
    let BackendState::Mimalloc(state) = &mut value.backend else {
        unreachable!()
    };
    state.owners[1].id = state.owners[0].id;
    assert_eq!(snapshot::encoded_len(&value).unwrap_err().kind(), ErrorKind::DuplicateOwner);

    let mut value = fixture();
    let BackendState::Mimalloc(state) = &mut value.backend else {
        unreachable!()
    };
    state.owners[0].source = ObservationSource::Unobserved;
    assert_eq!(snapshot::encoded_len(&value).unwrap_err().kind(), ErrorKind::Malformed);

    let mut value = fixture();
    let BackendState::Mimalloc(state) = &mut value.backend else {
        unreachable!()
    };
    state.owners[0].observation.as_mut().unwrap().classes[8].used_blocks = 129;
    assert_eq!(snapshot::encoded_len(&value).unwrap_err().kind(), ErrorKind::Malformed);
}

#[test]
fn malformed_owner_ids_tags_and_coverage_are_rejected_by_the_reader() {
    let valid = encoded(&fixture());
    // Schema 4: shared header/count/global state, then fixed-size owner rows
    // with an optional 5954-byte observation after each 11-byte row header.
    let first = 12 + 4 * 8 + 2 + 4 + 9 * 8 + 74 * 8 + 1;
    let second = first + 11 + 4 * 8 + 2 + 74 * 10 * 8;
    let mut duplicate = valid.clone();
    duplicate[second..second + 8].copy_from_slice(&16_u64.to_le_bytes());
    assert_eq!(snapshot::decode(&duplicate).unwrap_err().kind(), ErrorKind::DuplicateOwner);

    let mut unknown_tag = valid.clone();
    unknown_tag[first + 9] = 99;
    assert_eq!(snapshot::decode(&unknown_tag).unwrap_err().kind(), ErrorKind::Malformed);
    let mut invalid_count = valid.clone();
    invalid_count[36..44].copy_from_slice(&4_u64.to_le_bytes());
    assert_eq!(snapshot::decode(&invalid_count).unwrap_err().kind(), ErrorKind::Malformed);
    let mut invalid_id = fixture();
    let BackendState::Mimalloc(state) = &mut invalid_id.backend else {
        unreachable!()
    };
    state.owners[0].id = 0;
    assert_eq!(snapshot::encoded_len(&invalid_id).unwrap_err().kind(), ErrorKind::DuplicateOwner);

    let mut unsupported = valid;
    unsupported[10] = 99;
    assert_eq!(
        snapshot::decode(&unsupported).unwrap_err().to_string(),
        "native allocator payload: backend tag 99 is unsupported"
    );
}

#[test]
fn freshness_distinguishes_exited_missing_and_stale_observations() {
    let value = fixture();
    let BackendState::Mimalloc(state) = value.backend else {
        unreachable!()
    };
    let published = state.owners[0];
    let mut older_round = published;
    older_round.observation.as_mut().unwrap().round = 8;
    let mut newer = published;
    newer.observation.as_mut().unwrap().captured_nanos = 20;
    let mut older_session = published;
    older_session.observation.as_mut().unwrap().session_id = 6;

    let actual =
        [&published, &state.owners[1], &state.owners[2], &older_round, &newer, &older_session].map(|owner| owner.freshness(&value.header));

    assert_eq!(
        actual,
        [
            native::Freshness::Current,
            native::Freshness::PreviousLease,
            native::Freshness::Unknown,
            native::Freshness::PreviousRound,
            native::Freshness::NewerThanCapture,
            native::Freshness::PreviousSession,
        ]
    );
}
