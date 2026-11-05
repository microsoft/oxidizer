// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use seismograph::recorder::event::EventKind;

pub(crate) fn enable_recording() {
    static INITIALIZE: std::sync::Once = std::sync::Once::new();

    // Reconfiguring capacity restarts the process-wide session and can drop another test's events.
    INITIALIZE.call_once(|| {
        seismograph::recorder(seismograph::recorder::Configuration {
            general_events: seismograph::recorder::Policy::all(false),
            ..Default::default()
        });
    });
}

pub(crate) fn recorded_test_events(kind: EventKind) -> usize {
    let owner = seismograph::recorder::thread_id();
    seismograph::snapshot::decode(
        seismograph::snapshot(seismograph::snapshot::SnapshotOptions::default())
            .expect("an in-memory recorder snapshot must serialize")
            .as_bytes(),
    )
    .expect("the recorder must decode its own snapshot")
    .events
    .events
    .iter()
    .filter(|event| event.thread_id == owner && event.kind == kind)
    .count()
}
