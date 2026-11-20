// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use seismograph::recorder::event::EventKind;

pub(crate) fn enable_recording() {
    static INITIALIZE: std::sync::Once = std::sync::Once::new();

    // Reconfiguring capacity restarts the process-wide session and can drop another test's events.
    INITIALIZE.call_once(|| {
        #[cfg(miri)]
        let event_capacity = seismograph::recorder::EventBufferCapacity::new(64);
        #[cfg(not(miri))]
        let event_capacity = seismograph::recorder::EventBufferCapacity::default();
        seismograph::recorder(seismograph::recorder::Configuration {
            general_events: seismograph::recorder::Policy::all(false),
            event_capacity_per_thread: event_capacity,
            ..Default::default()
        });
    });
}

pub(crate) fn recorded_test_events(kind: EventKind) -> usize {
    seismograph::recorder::current_thread_events()
        .unwrap()
        .iter()
        .filter(|event| event.kind == kind)
        .count()
}
