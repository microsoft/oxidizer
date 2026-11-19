// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! I/O completion events across recording restarts.

use std::cell::Cell;

use seismograph::recorder::event::EventKind;
use seismograph::recorder::{Configuration, EventBufferCapacity, Policy};
use seismograph::snapshot::SnapshotOptions;
use seismograph_io::{BufferState, IoOutcome, IoResourceKind, Operation, Resource};

#[test]
fn completion_after_restart_keeps_its_identity_without_a_retained_start() {
    let configuration = Configuration {
        io: Policy::all(false),
        event_capacity_per_thread: EventBufferCapacity::new(64),
        ..Default::default()
    };
    seismograph::recorder(configuration);
    let resource = Resource::new(IoResourceKind::File);
    let operation = Operation::read_started(&resource, 64, BufferState::none);
    let original = seismograph::snapshot(SnapshotOptions::default()).unwrap();
    let original = seismograph::snapshot::decode(original.as_bytes()).unwrap();
    let original = original.events.events[0].io().unwrap();
    seismograph::recorder(Configuration::default());
    seismograph::recorder(configuration);
    let metadata_calls = Cell::new(0);

    operation.finish(64, IoOutcome::Success, || {
        metadata_calls.set(metadata_calls.get() + 1);
        BufferState::none()
    });
    let current = Operation::read_started(&resource, 8, BufferState::none);
    current.finish_without_buffer(8, IoOutcome::Success);

    let captured = seismograph::snapshot(SnapshotOptions::default()).unwrap();
    let decoded = seismograph::snapshot::decode(captured.as_bytes()).unwrap();
    seismograph::recorder(Configuration::default());
    let current = decoded.events.events[1].io().unwrap();
    assert_ne!(current.operation_id, original.operation_id);
    assert_eq!(
        (
            metadata_calls.get(),
            decoded
                .events
                .events
                .iter()
                .map(|event| {
                    let io = event.io().unwrap();
                    (event.kind, io.operation_id, io.resource_id, io.completed_bytes, io.outcome)
                })
                .collect::<Vec<_>>(),
        ),
        (
            1,
            vec![
                (
                    EventKind::IoReadFinished,
                    original.operation_id,
                    original.resource_id,
                    64,
                    IoOutcome::Success
                ),
                (
                    EventKind::IoReadStarted,
                    current.operation_id,
                    original.resource_id,
                    0,
                    IoOutcome::Pending
                ),
                (
                    EventKind::IoReadFinished,
                    current.operation_id,
                    original.resource_id,
                    8,
                    IoOutcome::Success
                ),
            ],
        )
    );
}
