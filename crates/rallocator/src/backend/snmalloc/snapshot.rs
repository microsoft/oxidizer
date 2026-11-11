// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Cooperative snmalloc snapshot source.

static SOURCE: seismograph::snapshot::Source = seismograph::snapshot::Source::new(
    seismograph_rallocator::source::ID,
    seismograph_rallocator::source::NAME,
    seismograph_rallocator::snapshot::SCHEMA_VERSION,
    capture,
);

pub(super) fn register_source() {
    // Owner creation holds the pool lock. Registration only links atomics.
    seismograph::snapshot::register_source(&SOURCE);
}

fn capture(_context: seismograph::snapshot::SnapshotContext<'_>) -> Result<seismograph::snapshot::SourceData, seismograph::Error> {
    seismograph::recorder::without_recording(capture_state)
}

fn capture_state() -> Result<seismograph::snapshot::SourceData, seismograph::Error> {
    let round = seismograph_rallocator::snapshot::observation_round();
    let captured_nanos = seismograph_rallocator::snapshot::captured_nanos();
    let (owners, owner_count, owners_complete) = super::thread::inventory(round, captured_nanos, super::observation::WALK_BUDGET)?;
    // Owner creation takes pool -> backend. Collection releases the pool first.
    let global = super::ranges::observe();
    let header = seismograph_rallocator::snapshot::Header {
        captured_nanos: seismograph_rallocator::snapshot::captured_nanos(),
        session_id: 0,
        round,
        owner_count,
        owners_complete,
        publication_enabled: seismograph_rallocator::snapshot::publication_enabled(),
    };
    let len = seismograph_rallocator::snapshot::encoded_len_snmalloc(&header, &global, owners.as_slice())
        .map_err(|_error| seismograph::Error::new("rallocator source sizing failed"))?;
    let mut data = seismograph::snapshot::SourceData::zeroed(len)?;
    seismograph_rallocator::snapshot::encode_snmalloc(&header, &global, owners.as_slice(), data.as_mut_bytes())
        .map_err(|_error| seismograph::Error::new("rallocator source encoding failed"))?;
    seismograph_rallocator::snapshot::request_observation();
    Ok(data)
}
