// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::cache::CacheName;

pub(super) fn record_event(tier_name: CacheName, fallback: bool, kind: seismograph::recorder::event::EventKind) {
    use seismograph::recorder::event::{EventClass, ObjectId, Record};

    seismograph::record(EventClass::Cache, || {
        let tier_id = ObjectId::new(cache_name_id(tier_name));
        Record::object_measurement(kind, tier_id, u64::from(fallback))
    });
}

fn cache_name_id(name: CacheName) -> u64 {
    const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

    name.as_bytes()
        .iter()
        .fold(FNV_OFFSET_BASIS, |hash, byte| (hash ^ u64::from(*byte)).wrapping_mul(FNV_PRIME))
}

#[cfg(test)]
mod tests {
    use seismograph::recorder::event::{EventClass, EventKind, ObjectId};
    use seismograph::recorder::{Configuration, RecordingPolicy};
    use seismograph::snapshot::{EventBufferDisposition, SnapshotOptions};
    use serial_test::serial;

    use super::*;

    #[test]
    #[serial]
    fn cache_events_are_gated_and_encode_tier_role() {
        const CACHE_NAME: CacheName = "seismograph-cache-event-test";

        seismograph::recorder(Configuration::default());
        let _ = seismograph::snapshot(SnapshotOptions {
            event_buffers: EventBufferDisposition::Release,
        });
        record_event(CACHE_NAME, false, EventKind::CacheHit);

        seismograph::recorder(Configuration {
            cache: RecordingPolicy::all(false),
            ..Configuration::default()
        });
        record_event(CACHE_NAME, false, EventKind::CacheHit);
        record_event(CACHE_NAME, true, EventKind::CacheMiss);

        let snapshot = seismograph::snapshot(SnapshotOptions {
            event_buffers: EventBufferDisposition::Release,
        })
        .unwrap();
        let decoded = seismograph::snapshot::decode(snapshot.as_bytes()).unwrap();
        let tier_id = ObjectId::new(cache_name_id(CACHE_NAME));
        let events = decoded
            .events
            .events
            .iter()
            .filter(|event| event.object_id() == Some(tier_id))
            .map(|event| (event.kind, event.measurement()))
            .collect::<Vec<_>>();

        assert_eq!(events, vec![(EventKind::CacheHit, Some(0)), (EventKind::CacheMiss, Some(1))]);
        assert_eq!(decoded.events.recording.cache, RecordingPolicy::all(false));
        assert_eq!(EventKind::CacheHit.class(), EventClass::Cache);
        seismograph::recorder(Configuration::default());
    }
}
