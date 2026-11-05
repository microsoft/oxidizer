// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! This executable owns its process-wide recorder configuration.

use seismograph::recorder::event::{EventClass, EventKind, EventPayload, ObjectId, Record};
use seismograph::recorder::{Configuration, EventBufferCapacity, Policy};
use seismograph::snapshot::{EventBufferDisposition, SnapshotOptions};

#[test]
fn gamma_round2_numeric_records_keep_the_requested_measurement_and_object() {
    seismograph::recorder(Configuration {
        general_events: Policy::all(false),
        event_capacity_per_thread: EventBufferCapacity::new(64),
        ..Configuration::default()
    });
    for (object, measurement) in [(31, 0), (37, 0x0102_0304_0506_0708), (41, u64::MAX)] {
        seismograph::record(EventClass::General, || {
            Some(Record::object_measurement(
                EventKind::ChannelHighWatermark,
                ObjectId::new(object),
                measurement,
            ))
        });
    }
    let bytes = seismograph::snapshot(SnapshotOptions {
        event_buffers: EventBufferDisposition::Stop,
    })
    .unwrap();
    let decoded = seismograph::snapshot::decode(bytes.as_bytes()).unwrap();
    let values = decoded
        .events
        .events
        .iter()
        .map(|event| {
            let EventPayload::Numeric(value) = event.payload else {
                panic!("measurement must retain its numeric payload");
            };
            (event.kind, value.object_id.get(), value.value)
        })
        .collect::<Vec<_>>();
    assert_eq!(
        values,
        [
            (EventKind::ChannelHighWatermark, 31, 0),
            (EventKind::ChannelHighWatermark, 37, 0x0102_0304_0506_0708),
            (EventKind::ChannelHighWatermark, 41, u64::MAX),
        ]
    );
}
