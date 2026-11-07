// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Deterministic runtime payload decoding regressions.

#[path = "support/gamma_wire.rs"]
mod wire;

use seismograph::recorder::event::{BacktraceCapture, EventTimestamp};
use seismograph::recorder::runtime::WorkerId;
use seismograph_runtime::snapshot::{CountersScope, ErrorKind, RuntimeState, TaskActivityState, TaskMetricsScope, WorkerState, decode};
use seismograph_runtime::worker::WorkerRole;

#[test]
fn gamma_survivor_schema_two_rejects_address_sections() {
    let mut bytes = b"SEISRUNT\x01\0\x02\0".to_vec();
    bytes.extend_from_slice(&0_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u32.to_le_bytes());
    bytes.extend_from_slice(&13_u64.to_le_bytes());
    for value in [0_u32, 0, u32::MAX, u32::MAX] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    assert_eq!(decode(&bytes).unwrap_err().kind(), ErrorKind::Malformed);
}

#[test]
fn gamma_survivor_runtime_minimum_records_are_admitted_at_exact_boundary() {
    let image = wire::image(2, [0; 6]);
    let mut bytes = image[..148].to_vec();
    bytes[52..68].fill(0);
    assert_eq!(decode(&bytes).unwrap().runtimes.len(), 1);
    bytes[60..64].copy_from_slice(&1_u32.to_le_bytes());
    let mut task = image[187..219].to_vec();
    task[24..28].fill(0);
    bytes.extend_from_slice(&task);
    assert_eq!(decode(&bytes).unwrap().runtimes[0].tasks.len(), 1);
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "The schema matrix checks every field against one independent wire fixture"
)]
fn gamma_survivor_runtime_schema_matrix_preserves_every_field() {
    for schema in 2..=6 {
        let bytes = wire::image(schema, [4, 100, 90, 80, 0, 2]);
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
        let mut changed_legacy_frames = bytes.clone();
        let frame_start = match schema {
            2 => 219,
            3 => 307,
            4 => 347,
            5 => 355,
            6 => 363,
            _ => unreachable!(),
        };
        changed_legacy_frames[frame_start..frame_start + 16].fill(0xa5);
        assert_eq!(decode(&changed_legacy_frames).unwrap(), decoded);
        assert_eq!(task.future_size_bytes, (schema >= 6).then_some(417));
        assert_eq!(task.spawned_at, (schema >= 3).then_some(EventTimestamp::from_ticks(11)));
        assert_eq!(task.metrics_scope, TaskMetricsScope::Lifetime);
        assert_eq!(task.last_worker_id.map(WorkerId::get), (schema >= 3).then_some(2));
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
            if schema >= 3 {
                (12, 13, 14, 15, 16, 17, 18, 19, 20)
            } else {
                (0, 0, 0, 0, 0, 0, 0, 0, 0)
            }
        );
        if schema >= 4 {
            let activity = task.activity.unwrap();
            assert_eq!(
                (
                    activity.state,
                    activity.observed_at.ticks(),
                    activity.ready_since.unwrap().ticks(),
                    activity.poll_started_at.unwrap().ticks()
                ),
                (TaskActivityState::Running, 100, 90, 80)
            );
            assert_eq!(activity.poll_worker_id.map(WorkerId::get), (schema >= 5).then_some(2));
            assert_eq!(activity.queued_since, None);
        } else {
            assert_eq!(task.activity, None);
        }
        if schema >= 3 {
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
        } else {
            assert!(decoded.addresses.is_empty());
        }
        for end in 0..bytes.len() {
            assert_eq!(
                decode(&bytes[..end]).unwrap_err().kind(),
                ErrorKind::Malformed,
                "schema {schema}, prefix {end}"
            );
        }
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
            decode(&wire::image(6, fields)).unwrap().runtimes[0].tasks[0]
                .activity
                .map(|activity| activity.state),
            state
        );
    }
}

#[test]
fn gamma_survivor_runtime_optional_sentinels_and_lifecycle_states() {
    let mut bytes = wire::image(6, [0; 6]);
    bytes[28] = 3;
    bytes[29] = 3;
    bytes[44..52].copy_from_slice(&101_u64.to_le_bytes());
    bytes[163] = 3;
    bytes[164] = 3;
    bytes[167..171].copy_from_slice(&u32::MAX.to_le_bytes());
    bytes[171..187].fill(0);
    bytes[195..203].fill(0);
    bytes[227..235].fill(0);
    bytes[355..363].fill(0);
    bytes[395..403].fill(255);
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
    bytes[163] = 2;
    bytes[164] = 2;
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
    let mut bytes = wire::image(6, [0; 6]);
    let long = 0x0807_0605_0403_0201_u64;
    let word = 0x0403_0201_u32;
    bytes[20..28].copy_from_slice(&long.to_le_bytes());
    bytes[36..44].copy_from_slice(&long.to_le_bytes());
    bytes[32..36].copy_from_slice(&word.to_le_bytes());
    bytes[167..171].copy_from_slice(&word.to_le_bytes());
    bytes[379..387].copy_from_slice(&long.to_le_bytes());
    bytes[395..399].copy_from_slice(&word.to_le_bytes());
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
    let image = wire::image(6, [4, 100, 90, 80, 0, 2]);
    let mut runtimes = image[..20].to_vec();
    runtimes[12..16].copy_from_slice(&2_u32.to_le_bytes());
    runtimes[16..20].fill(0);
    runtimes.extend_from_slice(&image[20..379]);
    let runtime_end = runtimes.len();
    runtimes.extend_from_slice(&image[20..379]);
    decode(&runtimes).unwrap();
    for end in runtime_end..runtimes.len() {
        assert_eq!(decode(&runtimes[..end]).unwrap_err().kind(), ErrorKind::Malformed);
    }
    let mut addresses = b"SEISRUNT\x01\0\x06\0".to_vec();
    addresses.extend_from_slice(&0_u32.to_le_bytes());
    addresses.extend_from_slice(&2_u32.to_le_bytes());
    addresses.extend_from_slice(&13_u64.to_le_bytes());
    for value in [128_u32, 128, 17, 19] {
        addresses.extend_from_slice(&value.to_le_bytes());
    }
    addresses.extend_from_slice(&[b'a'; 256]);
    let address_end = addresses.len();
    addresses.extend_from_slice(&image[379..]);
    decode(&addresses).unwrap();
    for end in address_end..addresses.len() {
        assert_eq!(decode(&addresses[..end]).unwrap_err().kind(), ErrorKind::Malformed);
    }
    let mut tasks = image[..379].to_vec();
    tasks[16..20].fill(0);
    tasks[60..64].copy_from_slice(&2_u32.to_le_bytes());
    let task_end = tasks.len();
    tasks.extend_from_slice(&image[187..379]);
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
        assert_eq!(decode(&wire::image(6, fields)).unwrap_err().kind(), ErrorKind::Malformed);
    }
}

#[test]
fn gamma_survivor_runtime_validation_and_error_text_are_exact() {
    let valid = wire::image(6, [4, 100, 90, 80, 0, 2]);
    for offset in [0, 20, 28, 29, 30, 64, 155, 163, 164, 165, 187, 203, 215, 417] {
        let mut invalid = valid.clone();
        invalid[offset] = if matches!(offset, 20 | 155 | 187 | 203) { 0 } else { 255 };
        assert_eq!(decode(&invalid).unwrap_err().kind(), ErrorKind::Malformed, "offset {offset}");
    }
    for offset in [12, 16, 52, 56, 60, 211] {
        let mut invalid = valid.clone();
        invalid[offset..offset + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(decode(&invalid).unwrap_err().kind(), ErrorKind::Malformed, "count offset {offset}");
    }
    for (offset, kind, text) in [
        (
            8,
            ErrorKind::UnsupportedWireVersion(9),
            "runtime telemetry wire version 9 is unsupported",
        ),
        (
            10,
            ErrorKind::UnsupportedSchemaVersion(9),
            "runtime telemetry schema version 9 is unsupported",
        ),
    ] {
        let mut invalid = valid.clone();
        invalid[offset..offset + 2].copy_from_slice(&9_u16.to_le_bytes());
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
fn historical_creation_at_tick_one_is_not_an_absent_timestamp() {
    for schema in 3..=6 {
        let mut bytes = wire::image(schema, [0; 6]);
        bytes[219..227].copy_from_slice(&1_u64.to_le_bytes());
        assert_eq!(
            decode(&bytes).unwrap().runtimes[0].tasks[0].spawned_at,
            Some(EventTimestamp::from_ticks(1))
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
        let mut bytes = wire::image(6, activity);
        bytes[10..12].copy_from_slice(&8_u16.to_le_bytes());
        bytes.splice(363..363, 0_u64.to_le_bytes());
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
