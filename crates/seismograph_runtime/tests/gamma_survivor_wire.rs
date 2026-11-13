// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Deterministic runtime payload decoding regressions.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

#[path = "support/gamma_wire.rs"]
mod wire;

use seismograph::recorder::event::{BacktraceCapture, EventTimestamp};
use seismograph::recorder::runtime::WorkerId;
use seismograph_runtime::snapshot::{CountersScope, ErrorKind, RuntimeState, TaskActivityState, TaskMetricsScope, WorkerState, decode};
use seismograph_runtime::worker::WorkerRole;

const WORKER_START: usize = 131;
const TASK_START: usize = 163;
const ADDRESS_START: usize = 347;

#[test]
fn historical_schemas_are_rejected_before_address_sections() {
    for schema in 0_u16..9 {
        let mut bytes = wire::image([0; 6]);
        bytes[10..12].copy_from_slice(&schema.to_le_bytes());
        assert_eq!(decode(&bytes).unwrap_err().kind(), ErrorKind::UnsupportedSchemaVersion(schema));
    }
}

#[test]
fn gamma_survivor_runtime_minimum_records_are_admitted_at_exact_boundary() {
    let image = wire::image([0; 6]);
    let mut bytes = image[..WORKER_START].to_vec();
    bytes[16..20].fill(0);
    bytes[56..64].fill(0);
    assert_eq!(decode(&bytes).unwrap().runtimes.len(), 1);
    bytes[60..64].copy_from_slice(&1_u32.to_le_bytes());
    bytes.extend_from_slice(&image[TASK_START..ADDRESS_START]);
    assert_eq!(decode(&bytes).unwrap().runtimes[0].tasks.len(), 1);
    bytes.pop();
    assert_eq!(decode(&bytes).unwrap_err().kind(), ErrorKind::Malformed);
}

#[test]
fn current_runtime_payload_preserves_every_field() {
    let bytes = wire::image([4, 100, 90, 80, 0, 2]);
    let decoded = decode(&bytes).unwrap();
    let runtime = &decoded.runtimes[0];
    assert_eq!(
        (runtime.id.get(), runtime.name.as_str(), runtime.configured_workers),
        (1, "primary", 4)
    );
    assert_eq!(
        (runtime.state, runtime.lifecycle_backtraces),
        (RuntimeState::Running, BacktraceCapture::Configured)
    );
    assert_eq!((runtime.created_at.ticks(), runtime.retired_at), (10, None));
    let counters = runtime.counters;
    assert_eq!(runtime.counters_scope, CountersScope::Lifetime);
    assert_eq!(
        (
            counters.spawned_tasks,
            counters.live_tasks,
            counters.completed_tasks,
            counters.canceled_tasks,
            counters.panicked_tasks,
            counters.poll_count,
            counters.poll_duration_nanos
        ),
        (7, 2, 21, 22, 23, 24, 25)
    );
    let worker = &runtime.workers[0];
    assert_eq!(
        (worker.id.get(), worker.role, worker.state, worker.processor_index),
        (2, WorkerRole::Core, WorkerState::Running, Some(3))
    );
    assert_eq!((worker.thread_id.unwrap().get(), worker.current_task.unwrap().get()), (9, 4));
    let task = &runtime.tasks[0];
    assert_eq!((task.id.get(), task.parent.unwrap().get(), task.type_descriptor.get()), (4, 3, 5));
    assert_eq!(task.future_size_bytes, Some(417));
    assert_eq!(task.spawned_at, Some(EventTimestamp::from_ticks(11)));
    assert_eq!(task.metrics_scope, TaskMetricsScope::Lifetime);
    assert_eq!(task.last_worker_id.map(WorkerId::get), Some(2));
    let metrics = task.metrics;
    assert_eq!(
        (
            metrics.poll_count,
            metrics.poll_duration_nanos,
            metrics.max_poll_duration_nanos,
            metrics.resume_count,
            metrics.resume_duration_nanos,
            metrics.max_resume_duration_nanos,
            metrics.ready_wait_count,
            metrics.ready_wait_duration_nanos,
            metrics.max_ready_wait_duration_nanos
        ),
        (12, 13, 14, 15, 16, 17, 18, 19, 20)
    );
    let activity = task.activity.unwrap();
    assert_eq!(
        (
            activity.state,
            activity.observed_at.ticks(),
            activity.ready_since.unwrap().ticks(),
            activity.poll_started_at.unwrap().ticks(),
            activity.poll_worker_id.map(WorkerId::get),
            activity.queued_since
        ),
        (TaskActivityState::Running, 100, 90, 80, Some(2), None)
    );
    let address = &decoded.addresses[0];
    assert_eq!(
        (
            address.address,
            address.symbol.as_deref(),
            address.filename.as_deref(),
            address.line,
            address.column
        ),
        (0x1234, Some("example::spawn"), Some("example.rs"), Some(42), Some(7))
    );
    for end in 0..bytes.len() {
        assert_eq!(decode(&bytes[..end]).unwrap_err().kind(), ErrorKind::Malformed, "prefix {end}");
    }
}

#[test]
fn gamma_survivor_runtime_activity_accepts_only_coherent_states() {
    for (fields, state) in [
        ([0, 0, 0, 0, 0, 0], None),
        ([1, 100, 0, 0, 0, 0], Some(TaskActivityState::Unknown)),
        ([2, 100, 0, 0, 0, 0], Some(TaskActivityState::Waiting)),
        ([3, 100, 80, 0, 90, 0], Some(TaskActivityState::Ready)),
        ([4, 100, 90, 80, 0, 2], Some(TaskActivityState::Running)),
    ] {
        assert_eq!(
            decode(&wire::image(fields)).unwrap().runtimes[0].tasks[0]
                .activity
                .map(|activity| activity.state),
            state
        );
    }
}

#[test]
fn gamma_survivor_runtime_optional_sentinels_and_lifecycle_states() {
    let mut bytes = wire::image([0; 6]);
    bytes[28] = 3;
    bytes[29] = 3;
    bytes[44..52].copy_from_slice(&101_u64.to_le_bytes());
    bytes[139] = 3;
    bytes[140] = 3;
    bytes[143..147].fill(255);
    bytes[147..163].fill(0);
    bytes[171..179].fill(0);
    bytes[203..211].fill(0);
    bytes[331..339].fill(0);
    bytes[363..371].fill(255);
    let runtime = &decode(&bytes).unwrap().runtimes[0];
    assert_eq!(runtime.state, RuntimeState::Stopped);
    assert_eq!(runtime.retired_at.unwrap().ticks(), 101);
    assert_eq!(runtime.lifecycle_backtraces, BacktraceCapture::Always);
    let worker = &runtime.workers[0];
    assert_eq!((worker.role, worker.state), (WorkerRole::Io, WorkerState::Stopped));
    assert_eq!((worker.processor_index, worker.thread_id, worker.current_task), (None, None, None));
    let task = &runtime.tasks[0];
    assert_eq!(
        (task.parent, task.last_worker_id, task.future_size_bytes, task.activity),
        (None, None, None, None)
    );
    assert_eq!(
        (
            decode(&bytes).unwrap().addresses[0].line,
            decode(&bytes).unwrap().addresses[0].column
        ),
        (None, None)
    );
    bytes[28] = 2;
    bytes[29] = 2;
    bytes[44..52].fill(0);
    bytes[139] = 2;
    bytes[140] = 2;
    let decoded = decode(&bytes).unwrap();
    assert_eq!(
        (decoded.runtimes[0].state, decoded.runtimes[0].lifecycle_backtraces),
        (RuntimeState::Stopping, BacktraceCapture::Never)
    );
    assert_eq!(
        (decoded.runtimes[0].workers[0].role, decoded.runtimes[0].workers[0].state),
        (WorkerRole::Blocking, WorkerState::Parked)
    );
}

#[test]
fn gamma_survivor_runtime_multibyte_scalars_use_every_little_endian_byte() {
    let mut bytes = wire::image([0; 6]);
    let long = 0x0807_0605_0403_0201_u64;
    let word = 0x0403_0201_u32;
    bytes[20..28].copy_from_slice(&long.to_le_bytes());
    bytes[36..44].copy_from_slice(&long.to_le_bytes());
    bytes[32..36].copy_from_slice(&word.to_le_bytes());
    bytes[143..147].copy_from_slice(&word.to_le_bytes());
    bytes[347..355].copy_from_slice(&long.to_le_bytes());
    bytes[363..367].copy_from_slice(&word.to_le_bytes());
    let decoded = decode(&bytes).unwrap();
    let runtime = &decoded.runtimes[0];
    assert_eq!((runtime.id.get(), runtime.created_at.ticks()), (long, long));
    assert_eq!(runtime.configured_workers, word);
    assert_eq!(runtime.workers[0].processor_index, Some(word));
    assert_eq!(decoded.addresses[0].address, long);
    assert_eq!(decoded.addresses[0].line, Some(word));
}

#[test]
fn gamma_survivor_runtime_variable_records_reach_later_truncations() {
    let image = wire::image([4, 100, 90, 80, 0, 2]);
    let mut runtimes = image[..20].to_vec();
    runtimes[12..16].copy_from_slice(&2_u32.to_le_bytes());
    runtimes[16..20].fill(0);
    runtimes.extend_from_slice(&image[20..ADDRESS_START]);
    let runtime_end = runtimes.len();
    runtimes.extend_from_slice(&image[20..ADDRESS_START]);
    decode(&runtimes).unwrap();
    for end in runtime_end..runtimes.len() {
        assert_eq!(decode(&runtimes[..end]).unwrap_err().kind(), ErrorKind::Malformed);
    }
    let mut addresses = b"SEISRUNT\x01\0\x09\0".to_vec();
    addresses.extend_from_slice(&0_u32.to_le_bytes());
    addresses.extend_from_slice(&2_u32.to_le_bytes());
    addresses.extend_from_slice(&13_u64.to_le_bytes());
    for value in [128_u32, 128, 17, 19] {
        addresses.extend_from_slice(&value.to_le_bytes());
    }
    addresses.extend_from_slice(&[b'a'; 256]);
    let address_end = addresses.len();
    addresses.extend_from_slice(&image[ADDRESS_START..]);
    decode(&addresses).unwrap();
    for end in address_end..addresses.len() {
        assert_eq!(decode(&addresses[..end]).unwrap_err().kind(), ErrorKind::Malformed);
    }
    let mut tasks = image[..ADDRESS_START].to_vec();
    tasks[16..20].fill(0);
    tasks[60..64].copy_from_slice(&2_u32.to_le_bytes());
    let task_end = tasks.len();
    tasks.extend_from_slice(&image[TASK_START..ADDRESS_START]);
    decode(&tasks).unwrap();
    for end in task_end..tasks.len() {
        assert_eq!(decode(&tasks[..end]).unwrap_err().kind(), ErrorKind::Malformed);
    }
}

#[test]
fn gamma_survivor_runtime_rejects_incoherent_activity_fields() {
    for fields in [
        [0, 1, 0, 0, 0, 0],
        [0, 0, 1, 0, 0, 0],
        [0, 0, 0, 1, 0, 0],
        [0, 0, 0, 0, 1, 0],
        [1, 100, 1, 0, 0, 0],
        [1, 100, 0, 1, 0, 0],
        [1, 100, 0, 0, 1, 0],
        [2, 100, 0, 1, 0, 0],
        [2, 100, 1, 0, 0, 0],
        [2, 100, 0, 0, 1, 0],
        [3, 100, 0, 0, 90, 0],
        [3, 100, 90, 0, 80, 0],
        [3, 100, 80, 1, 90, 0],
        [3, 100, 80, 0, 90, 2],
        [4, 100, 70, 80, 0, 2],
        [4, 100, 90, 80, 1, 2],
        [4, 100, 90, 0, 0, 2],
        [4, 100, 101, 80, 0, 2],
        [4, 100, 0, 101, 0, 2],
        [9, 100, 0, 0, 0, 0],
    ] {
        assert_eq!(decode(&wire::image(fields)).unwrap_err().kind(), ErrorKind::Malformed);
    }
}

#[test]
fn gamma_survivor_runtime_validation_and_error_text_are_exact() {
    let valid = wire::image([4, 100, 90, 80, 0, 2]);
    for offset in [0, 20, 28, 29, 30, 64, 131, 139, 140, 141, 163, 179, 191, 385] {
        let mut invalid = valid.clone();
        invalid[offset] = if matches!(offset, 20 | 131 | 163 | 179) { 0 } else { 255 };
        assert_eq!(decode(&invalid).unwrap_err().kind(), ErrorKind::Malformed, "offset {offset}");
    }
    for offset in [12, 16, 52, 56, 60, 187] {
        let mut invalid = valid.clone();
        invalid[offset..offset + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(decode(&invalid).unwrap_err().kind(), ErrorKind::Malformed, "count offset {offset}");
    }
    for (offset, kind, text) in [
        (
            8,
            ErrorKind::UnsupportedWireVersion(99),
            "runtime telemetry wire version 99 is unsupported",
        ),
        (
            10,
            ErrorKind::UnsupportedSchemaVersion(99),
            "runtime telemetry schema version 99 is unsupported",
        ),
    ] {
        let mut invalid = valid.clone();
        invalid[offset..offset + 2].copy_from_slice(&99_u16.to_le_bytes());
        let error = decode(&invalid).unwrap_err();
        assert_eq!(error.kind(), kind);
        assert_eq!(error.to_string(), text);
    }
    assert_eq!(
        decode(&[]).unwrap_err().to_string(),
        "the runtime telemetry source payload is malformed"
    );
    assert_eq!(
        EventTimestamp::from_ticks(100)
            .duration_since(EventTimestamp::from_ticks(101))
            .as_nanos(),
        0
    );
}

#[test]
fn creation_at_tick_zero_or_one_is_not_an_absent_timestamp() {
    for tick in [0_u64, 1] {
        let mut bytes = wire::image([0; 6]);
        bytes[195..203].copy_from_slice(&(tick + 1).to_le_bytes());
        assert_eq!(
            decode(&bytes).unwrap().runtimes[0].tasks[0].spawned_at,
            Some(EventTimestamp::from_ticks(tick))
        );
    }
}

#[test]
fn native_ready_tasks_require_both_queue_ages_or_neither() {
    for (activity, valid) in [
        ([3, 100, 0, 0, 0, 0], true),
        ([3, 100, 80, 0, 90, 0], true),
        ([3, 100, 0, 0, 90, 0], false),
        ([3, 100, 90, 0, 0, 0], false),
    ] {
        let bytes = wire::image(activity);
        if valid {
            assert_eq!(
                decode(&bytes).unwrap().runtimes[0].tasks[0].activity.unwrap().state,
                TaskActivityState::Ready
            );
        } else {
            assert_eq!(decode(&bytes).unwrap_err().kind(), ErrorKind::Malformed);
        }
    }
}
