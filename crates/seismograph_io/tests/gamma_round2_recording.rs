// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! This executable owns its process-wide recorder configuration.

use std::cell::Cell;

use seismograph::recorder::event::EventKind;
use seismograph::recorder::io::IoEvent;
use seismograph::recorder::{Configuration, EventBufferCapacity, Policy};
use seismograph::snapshot::{EventBufferDisposition, SnapshotOptions};
use seismograph_io::{BufferId, BufferState, IoOutcome, IoResourceKind, Operation, Resource};

#[test]
fn gamma_round2_io_pairs_keep_requests_completion_metadata_and_cancellation() {
    seismograph::recorder(Configuration {
        io: Policy::all(false),
        event_capacity_per_thread: EventBufferCapacity::new(64),
        ..Configuration::default()
    });
    let resource = Resource::new(IoResourceKind::File);
    let buffer_id = BufferId::allocate();
    let calls = Cell::new(0);
    let read = Operation::read_started(&resource, 53, || {
        calls.set(calls.get() + 1);
        BufferState::new(Some(buffer_id), 41, 3)
    });
    assert!(read.was_recorded());
    read.finish(19, IoOutcome::Success, || {
        calls.set(calls.get() + 1);
        BufferState::new(Some(buffer_id), 7, 2)
    });
    assert_eq!(calls.get(), 2);
    let write = Operation::write_started(&resource, 61, || BufferState::new(Some(buffer_id), 101, 7));
    assert!(write.was_recorded());
    write.finish_without_buffer(29, IoOutcome::Success);
    let canceled = Operation::read_started(&resource, 71, BufferState::none);
    assert!(canceled.was_recorded());
    drop(canceled);
    let bytes = seismograph::snapshot(SnapshotOptions {
        event_buffers: EventBufferDisposition::Stop,
    })
    .unwrap();
    let decoded = seismograph::snapshot::decode(bytes.as_bytes()).unwrap();
    let actual = decoded
        .events
        .events
        .iter()
        .map(|event| (event.kind, event.io().unwrap()))
        .collect::<Vec<_>>();
    assert_eq!(actual.len(), 6);
    let resource_id = actual[0].1.resource_id;
    let expected = [
        (EventKind::IoReadStarted, 53, 0, Some(buffer_id), 41, 3, IoOutcome::Pending),
        (EventKind::IoReadFinished, 53, 19, Some(buffer_id), 7, 2, IoOutcome::Success),
        (EventKind::IoWriteStarted, 61, 0, Some(buffer_id), 101, 7, IoOutcome::Pending),
        (EventKind::IoWriteFinished, 61, 29, Some(buffer_id), 101, 7, IoOutcome::Success),
        (EventKind::IoReadStarted, 71, 0, None, 0, 0, IoOutcome::Pending),
        (EventKind::IoReadFinished, 71, 0, None, 0, 0, IoOutcome::Canceled),
    ];
    for (index, ((kind, event), (expected_kind, requested_bytes, completed_bytes, id, buffer_len, buffer_span_count, outcome))) in
        actual.iter().zip(expected).enumerate()
    {
        assert_eq!(*kind, expected_kind);
        assert_eq!(
            *event,
            IoEvent {
                operation_id: actual[index & !1].1.operation_id,
                resource_id,
                buffer_id: id,
                requested_bytes,
                completed_bytes,
                buffer_len,
                buffer_span_count,
                resource_kind: IoResourceKind::File,
                outcome,
            }
        );
    }
    assert_ne!(actual[0].1.operation_id, actual[2].1.operation_id);
    assert_ne!(actual[2].1.operation_id, actual[4].1.operation_id);
}
