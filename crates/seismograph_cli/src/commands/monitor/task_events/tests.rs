// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use seismograph::recorder::alloc::{Allocation, AllocationId, EventThreadId, HeapId, HeapKind};
use seismograph::recorder::event::{Address, EventClock, EventSequence, EventTimestamp, NumericEvent, ObjectId};
use seismograph::recorder::runtime::{RuntimeEvent, RuntimeId, TaskId, TypeDescriptorId, WorkerId};
use seismograph::recorder::thread::{ThreadId, ThreadLog};
use seismograph::recorder::{RecordingPolicies, RecordingPolicy};
use seismograph::snapshot::DecodedSnapshot;
use seismograph_rallocator::callers::AddressLookupFields;
use seismograph_runtime::snapshot::{Counters, Runtime, RuntimeState, Task, TaskActivity, TaskActivityState, TaskMetrics};

use super::super::filter::{FilterSpec, RuntimeStackMode};
use super::super::filter_index::FilterIndex;
use super::*;

fn operation(thread: u64, sequence: u64, timestamp: u64, kind: EventKind, object: u64) -> Event {
    Event {
        thread_id: ThreadId::new(thread),
        sequence: EventSequence::new(sequence),
        timestamp: EventTimestamp::from_ticks(timestamp),
        kind,
        payload: EventPayload::Object(ObjectId::new(object)),
        call_stack: Vec::new(),
    }
}

fn poll(thread: u64, sequence: u64, timestamp: u64, task: TaskKey, kind: EventKind, duration: u64) -> Event {
    Event {
        payload: EventPayload::Runtime(RuntimeEvent {
            runtime_id: RuntimeId::from_raw(task.0).unwrap(),
            worker_id: WorkerId::from_raw(thread + 100),
            subject_id: task.1,
            related_id: 0,
            value_0: duration,
            value_1: 0,
        }),
        ..operation(thread, sequence, timestamp, kind, 0)
    }
}

fn start(thread: u64, sequence: u64, timestamp: u64, task: TaskKey) -> Event {
    poll(thread, sequence, timestamp, task, EventKind::TaskPollStarted, 0)
}

fn finish(thread: u64, sequence: u64, timestamp: u64, task: TaskKey, duration: u64) -> Event {
    poll(thread, sequence, timestamp, task, EventKind::TaskPollFinished, duration)
}

fn events(events: Vec<Event>) -> Events {
    let mut totals = BTreeMap::<u64, u64>::new();
    for event in &events {
        totals
            .entry(event.thread_id.get())
            .and_modify(|total| *total = (*total).max(event.sequence.get()))
            .or_insert_with(|| event.sequence.get());
    }
    Events {
        clock: EventClock::ProcessMonotonic,
        total_events: totals.values().sum(),
        recording: RecordingPolicies {
            runtime_tasks: RecordingPolicy::all(true),
            general_events: RecordingPolicy::all(true),
            allocations: RecordingPolicy::all(true),
            ..RecordingPolicies::default()
        },
        threads: totals
            .into_iter()
            .map(|(id, total_events)| ThreadLog {
                thread_id: ThreadId::new(id),
                total_events,
                ..ThreadLog::default()
            })
            .collect(),
        events,
        ..Events::default()
    }
}

fn summarize(events: Vec<Event>) -> TaskEventsSnapshot {
    TaskEventsSnapshot::from_events(&self::events(events), &[], None)
}

fn actors(snapshot: &TaskEventsSnapshot) -> Vec<(u64, u64, Option<TaskKey>, bool)> {
    let mut actors = snapshot
        .histories
        .iter()
        .flat_map(|history| history.events.iter())
        .map(|event| (event.thread_id, event.sequence, event.task, event.ambiguous))
        .collect::<Vec<_>>();
    actors.sort_unstable();
    actors
}

fn allocation(thread: u64, sequence: u64, timestamp: u64, kind: EventKind, id: u64) -> Event {
    Event {
        payload: EventPayload::Allocation(Allocation {
            allocation_id: AllocationId::new(id),
            event_thread_id: EventThreadId::new(thread + 1_000),
            heap_id: HeapId::new(8),
            heap_kind: HeapKind::General,
            freed_after_heap_release: false,
            address: Address::new(0x8000),
            size: 64,
            alignment: 16,
        }),
        ..operation(thread, sequence, timestamp, kind, id)
    }
}

#[test]
fn occurrence_index_is_actor_and_operation_specific_chronological_and_filterable() {
    let snapshot = summarize(vec![
        start(1, 1, 100, (1, 10)),
        operation(1, 2, 200, EventKind::MutexAccess, 9),
        operation(1, 3, 220, EventKind::MutexRelease, 9),
        operation(1, 4, 230, EventKind::MutexAccess, 8),
        finish(1, 5, 300, (1, 10), 200),
        start(1, 6, 310, (1, 20)),
        operation(1, 7, 320, EventKind::MutexAccess, 9),
        finish(1, 8, 350, (1, 20), 40),
        operation(1, 9, 600, EventKind::MutexAccess, 9),
        start(2, 1, 400, (1, 10)),
        operation(2, 2, 450, EventKind::MutexAccess, 9),
        operation(2, 3, 450, EventKind::MutexAccess, 7),
        finish(2, 4, 500, (1, 10), 100),
    ]);
    let selected = &snapshot.tasks[&(1, 10)]
        .operations
        .iter()
        .find(|operation| operation.kind.event_kind() == EventKind::MutexAccess)
        .unwrap();
    let shown = selected
        .occurrences()
        .map(|(object, event)| {
            assert_eq!((event.task, event.kind), (Some((1, 10)), EventKind::MutexAccess));
            (event.timestamp, event.thread_id, event.sequence, object.object_id)
        })
        .collect::<Vec<_>>();
    assert_eq!(
        (selected.events, shown),
        (4, vec![(200, 1, 2, 9), (230, 1, 4, 8), (450, 2, 2, 9), (450, 2, 3, 7)])
    );
    assert_eq!(
        selected.occurrence(2).map(|(_, event)| (event.thread_id, event.sequence)),
        Some((2, 2))
    );
    assert_eq!(selected.occurrence(4), None);
    let filtered = snapshot.filtered(&HashSet::from([(1, 4), (2, 2), (1, 7)]));
    let selected = &filtered.tasks[&(1, 10)].operations[0];
    assert_eq!(
        (
            selected.events,
            selected
                .occurrences()
                .map(|(object, event)| (event.timestamp, object.object_id))
                .collect::<Vec<_>>()
        ),
        (2, vec![(230, 8), (450, 9)]),
    );
}

#[test]
fn sequential_tasks_exclude_outside_events_and_ready_notifiers() {
    let snapshot = summarize(vec![
        operation(1, 1, 1, EventKind::MutexAccess, 7),
        start(1, 2, 2, (1, 10)),
        operation(1, 3, 3, EventKind::MutexAccess, 7),
        poll(1, 4, 4, (1, 20), EventKind::TaskReady, 0),
        operation(1, 5, 5, EventKind::MutexRelease, 7),
        finish(1, 6, 6, (1, 10), 4),
        start(1, 7, 7, (1, 20)),
        operation(1, 8, 8, EventKind::MutexAccess, 7),
        finish(1, 9, 9, (1, 20), 2),
        operation(1, 10, 10, EventKind::MutexRelease, 7),
    ]);
    assert_eq!(
        (
            actors(&snapshot),
            snapshot.tasks[&(1, 10)].inferred_events,
            snapshot.tasks[&(1, 20)].inferred_events,
            snapshot.unassigned_events
        ),
        (
            vec![
                (1, 1, None, false),
                (1, 3, Some((1, 10)), false),
                (1, 5, Some((1, 10)), false),
                (1, 8, Some((1, 20)), false),
                (1, 10, None, false)
            ],
            2,
            1,
            2
        ),
    );
}

#[test]
fn migration_uses_actual_recorder_threads_and_composite_task_identity() {
    let mut raw = vec![
        start(42, 1, 1, (1, 9)),
        operation(42, 2, 2, EventKind::ArcClone, 7),
        finish(42, 3, 3, (1, 9), 2),
        start(81, 1, 4, (1, 9)),
        operation(81, 2, 5, EventKind::ArcClone, 7),
        finish(81, 3, 6, (1, 9), 2),
        start(42, 4, 7, (2, 9)),
        operation(42, 5, 8, EventKind::ArcClone, 7),
        finish(42, 6, 9, (2, 9), 2),
    ];
    for event in &mut raw {
        if let EventPayload::Runtime(runtime) = &mut event.payload {
            runtime.worker_id = WorkerId::from_raw(500);
        }
    }
    raw.reverse();
    let snapshot = summarize(raw);
    assert_eq!(
        (
            actors(&snapshot),
            snapshot
                .tasks
                .iter()
                .map(|(key, task)| (*key, task.inferred_events))
                .collect::<Vec<_>>()
        ),
        (
            vec![
                (42, 2, Some((1, 9)), false),
                (42, 5, Some((2, 9)), false),
                (81, 2, Some((1, 9)), false)
            ],
            vec![((1, 9), 2), ((2, 9), 1)]
        ),
    );
}

#[test]
fn retained_sequences_disambiguate_equal_timestamps() {
    let snapshot = summarize(vec![
        operation(1, 1, 7, EventKind::ArcClone, 7),
        start(1, 2, 7, (1, 1)),
        operation(1, 3, 7, EventKind::ArcClone, 7),
        finish(1, 4, 7, (1, 1), 0),
        start(1, 5, 7, (1, 2)),
        operation(1, 6, 7, EventKind::ArcClone, 7),
        finish(1, 7, 7, (1, 2), 0),
        operation(1, 8, 7, EventKind::ArcClone, 7),
    ]);
    assert_eq!(
        actors(&snapshot),
        vec![
            (1, 1, None, false),
            (1, 3, Some((1, 1)), false),
            (1, 6, Some((1, 2)), false),
            (1, 8, None, false)
        ]
    );
}

#[test]
fn finish_only_recovers_a_continuous_suffix_but_not_a_start_timestamp_tie() {
    let snapshot = summarize(vec![
        operation(1, 40, 10, EventKind::MutexAccess, 7),
        operation(1, 41, 11, EventKind::MutexAccess, 7),
        operation(1, 42, 12, EventKind::MutexRelease, 7),
        finish(1, 43, 15, (1, 1), 5),
    ]);
    assert_eq!(
        actors(&snapshot),
        vec![(1, 40, None, false), (1, 41, Some((1, 1)), false), (1, 42, Some((1, 1)), false)]
    );
}

#[test]
fn unmatched_start_does_not_claim_arbitrary_later_work() {
    let snapshot = summarize(vec![
        start(1, 1, 1, (1, 1)),
        operation(1, 2, 2, EventKind::MutexAccess, 7),
        operation(1, 3, 90, EventKind::MutexRelease, 7),
    ]);
    assert_eq!(
        (snapshot.tasks.len(), snapshot.unassigned_events, snapshot.ambiguous_events),
        (0, 2, 0)
    );
}

#[test]
fn retained_nested_polls_are_never_double_assigned() {
    let snapshot = summarize(vec![
        start(1, 1, 1, (1, 1)),
        operation(1, 2, 2, EventKind::MutexAccess, 7),
        start(1, 3, 3, (1, 2)),
        operation(1, 4, 4, EventKind::MutexAccess, 7),
        finish(1, 5, 5, (1, 2), 2),
        operation(1, 6, 6, EventKind::MutexRelease, 7),
        finish(1, 7, 7, (1, 1), 6),
    ]);
    assert_eq!(
        actors(&snapshot),
        vec![(1, 2, Some((1, 1)), false), (1, 4, None, true), (1, 6, Some((1, 1)), false)]
    );
}

#[test]
fn crossing_polls_invalidate_the_overlapping_cluster() {
    let snapshot = summarize(vec![
        start(1, 1, 1, (1, 1)),
        operation(1, 2, 2, EventKind::MutexAccess, 7),
        start(1, 3, 3, (1, 2)),
        operation(1, 4, 4, EventKind::MutexAccess, 7),
        finish(1, 5, 5, (1, 1), 4),
        operation(1, 6, 6, EventKind::MutexRelease, 7),
        finish(1, 7, 7, (1, 2), 4),
    ]);
    assert_eq!((snapshot.tasks.len(), snapshot.ambiguous_events), (0, 3));
}

#[test]
fn missing_nested_start_is_not_hidden_by_outer_finish_duration() {
    let snapshot = summarize(vec![
        operation(1, 20, 12, EventKind::MutexAccess, 7),
        finish(1, 21, 15, (1, 2), 5),
        operation(1, 22, 17, EventKind::MutexRelease, 7),
        finish(1, 23, 20, (1, 1), 19),
    ]);
    assert_eq!(actors(&snapshot), vec![(1, 20, None, true), (1, 22, Some((1, 1)), false)]);
}

#[test]
fn missing_nested_finish_invalidates_an_enclosing_poll() {
    let snapshot = summarize(vec![
        start(1, 1, 1, (1, 1)),
        start(1, 2, 2, (1, 2)),
        operation(1, 3, 3, EventKind::MutexAccess, 7),
        finish(1, 4, 4, (1, 1), 3),
    ]);
    assert_eq!((snapshot.tasks.len(), snapshot.ambiguous_events), (0, 1));
}

#[test]
fn gaps_can_hide_nested_polls_even_when_both_outer_boundaries_survive() {
    let snapshot = summarize(vec![
        start(1, 1, 1, (1, 1)),
        operation(1, 2, 2, EventKind::MutexAccess, 7),
        operation(1, 8, 8, EventKind::MutexRelease, 7),
        finish(1, 9, 9, (1, 1), 8),
    ]);
    assert_eq!((snapshot.tasks.len(), snapshot.ambiguous_events), (0, 2));
}

#[test]
fn finish_only_does_not_bridge_a_sequence_gap() {
    let snapshot = summarize(vec![
        operation(1, 20, 12, EventKind::MutexAccess, 7),
        operation(1, 22, 14, EventKind::MutexRelease, 7),
        finish(1, 23, 15, (1, 1), 5),
    ]);
    assert_eq!((snapshot.tasks.len(), snapshot.ambiguous_events), (0, 2));
}

#[test]
fn runtime_sampling_cannot_be_detected_from_contiguous_sequences() {
    let mut raw = events(vec![
        start(1, 1, 1, (1, 1)),
        operation(1, 2, 2, EventKind::MutexAccess, 7),
        finish(1, 3, 3, (1, 1), 2),
    ]);
    raw.recording.runtime_tasks.event_sampling = seismograph::recorder::EventSampling::one_in(2).unwrap();
    let snapshot = TaskEventsSnapshot::from_events(&raw, &[], None);
    assert_eq!((snapshot.tasks.len(), snapshot.ambiguous_events), (0, 1));
}

#[test]
fn legacy_unknown_recording_policy_is_not_assumed_unsampled() {
    let mut raw = events(vec![
        start(1, 1, 1, (1, 1)),
        operation(1, 2, 2, EventKind::MutexAccess, 7),
        finish(1, 3, 3, (1, 1), 2),
    ]);
    raw.recording = RecordingPolicies::default();
    let snapshot = TaskEventsSnapshot::from_events(&raw, &[], None);
    assert_eq!((snapshot.tasks.len(), snapshot.ambiguous_events), (0, 1));
}

#[test]
fn sampled_operations_and_loss_outside_a_complete_poll_do_not_hide_boundaries() {
    let mut raw = events(vec![
        start(1, 100, 100, (1, 1)),
        operation(1, 101, 101, EventKind::MutexAccess, 7),
        finish(1, 102, 102, (1, 1), 2),
    ]);
    raw.recording.general_events.event_sampling = seismograph::recorder::EventSampling::one_in(32).unwrap();
    raw.lost_events = 99;
    raw.threads[0].lost_events = 99;
    let snapshot = TaskEventsSnapshot::from_events(&raw, &[], None);
    assert_eq!(actors(&snapshot), vec![(1, 101, Some((1, 1)), false)]);
}

#[test]
fn reversed_event_time_invalidates_an_otherwise_contiguous_poll() {
    let snapshot = summarize(vec![
        start(1, 1, 10, (1, 1)),
        operation(1, 2, 9, EventKind::MutexAccess, 7),
        finish(1, 3, 12, (1, 1), 2),
    ]);
    assert_eq!((snapshot.tasks.len(), snapshot.ambiguous_events), (0, 1));
}

#[test]
fn duplicate_sequences_cannot_prove_poll_continuity() {
    let snapshot = summarize(vec![
        start(1, 1, 1, (1, 1)),
        operation(1, 2, 2, EventKind::MutexAccess, 7),
        operation(1, 2, 2, EventKind::MutexRelease, 7),
        finish(1, 3, 3, (1, 1), 2),
    ]);
    assert_eq!((snapshot.tasks.len(), snapshot.ambiguous_events), (0, 2));
}

#[test]
fn mismatched_worker_or_duration_does_not_make_a_trustworthy_pair() {
    let mut end = finish(1, 3, 3, (1, 1), 2);
    if let EventPayload::Runtime(runtime) = &mut end.payload {
        runtime.worker_id = WorkerId::from_raw(999);
    }
    let worker = summarize(vec![start(1, 1, 1, (1, 1)), operation(1, 2, 2, EventKind::MutexAccess, 7), end]);
    let duration = summarize(vec![
        start(1, 1, 1, (1, 1)),
        operation(1, 2, 2, EventKind::MutexAccess, 7),
        finish(1, 3, 3, (1, 1), 1),
    ]);
    assert_eq!(
        (
            worker.tasks.len(),
            worker.ambiguous_events,
            duration.tasks.len(),
            duration.ambiguous_events
        ),
        (0, 1, 0, 1)
    );
}

#[test]
fn simultaneous_polls_of_one_task_on_distinct_threads_are_ambiguous() {
    let snapshot = summarize(vec![
        start(1, 1, 1, (1, 1)),
        operation(1, 2, 3, EventKind::MutexAccess, 7),
        finish(1, 3, 5, (1, 1), 4),
        start(2, 1, 2, (1, 1)),
        operation(2, 2, 4, EventKind::MutexAccess, 7),
        finish(2, 3, 6, (1, 1), 4),
    ]);
    assert_eq!((snapshot.tasks.len(), snapshot.ambiguous_events), (0, 2));
}

fn active_source() -> Snapshot {
    Snapshot {
        runtimes: vec![Runtime {
            id: RuntimeId::from_raw(1).unwrap(),
            name: "runtime".into(),
            configured_workers: 1,
            lifecycle_backtraces: seismograph::recorder::event::BacktraceCapture::Never,
            state: RuntimeState::Running,
            created_at: EventTimestamp::from_ticks(0),
            retired_at: None,
            counters: Counters::default(),
            workers: Vec::new(),
            tasks: vec![Task {
                id: TaskId::from_raw(1).unwrap(),
                parent: None,
                type_descriptor: TypeDescriptorId::from_raw(1).unwrap(),
                spawned_at: EventTimestamp::from_ticks(0),
                last_worker_id: None,
                metrics: TaskMetrics::default(),
                activity: Some(TaskActivity {
                    observed_at: EventTimestamp::from_ticks(10),
                    state: TaskActivityState::Running,
                    ready_since: None,
                    poll_started_at: Some(EventTimestamp::from_ticks(1)),
                    poll_worker_id: WorkerId::from_raw(101),
                    queued_since: None,
                }),
                spawn_backtrace: vec![Address::new(900)],
                future_size_bytes: None,
            }],
        }],
        addresses: Vec::new(),
    }
}

#[test]
fn coherent_activity_bounds_an_open_poll_using_the_retained_start_thread() {
    let raw = events(vec![
        start(1, 1, 1, (1, 1)),
        operation(1, 2, 2, EventKind::MutexAccess, 7),
        operation(1, 3, 10, EventKind::MutexRelease, 7),
        operation(1, 4, 12, EventKind::MutexAccess, 7),
        operation(2, 1, 3, EventKind::MutexAccess, 7),
    ]);
    let snapshot = TaskEventsSnapshot::from_events(&raw, &[], Some(&active_source()));
    assert_eq!(
        actors(&snapshot),
        vec![
            (1, 2, Some((1, 1)), false),
            (1, 3, None, false),
            (1, 4, None, false),
            (2, 1, None, false)
        ]
    );
}

#[test]
fn activity_without_a_retained_start_never_invents_the_recorder_thread() {
    let snapshot = TaskEventsSnapshot::from_events(
        &events(vec![operation(1, 2, 2, EventKind::MutexAccess, 7)]),
        &[],
        Some(&active_source()),
    );
    assert_eq!((snapshot.tasks.len(), snapshot.unassigned_events), (0, 1));
}

#[test]
fn stopped_runtime_without_task_metadata_uses_the_retained_recording_policy() {
    let raw = events(vec![
        start(1, 1, 1, (1, 1)),
        operation(1, 2, 2, EventKind::MutexAccess, 7),
        finish(1, 3, 3, (1, 1), 2),
    ]);
    let mut source = active_source();
    source.runtimes[0].state = RuntimeState::Stopped;
    source.runtimes[0].retired_at = Some(EventTimestamp::from_ticks(3));
    source.runtimes[0].tasks.clear();
    let snapshot = TaskEventsSnapshot::from_events(&raw, &[], Some(&source));
    assert_eq!(actors(&snapshot), vec![(1, 2, Some((1, 1)), false)]);
}

#[test]
fn open_poll_requires_a_complete_retained_tail_at_the_source_observation() {
    let mut raw = events(vec![start(1, 1, 1, (1, 1)), operation(1, 2, 2, EventKind::MutexAccess, 7)]);
    raw.threads[0].total_events = 3;
    let snapshot = TaskEventsSnapshot::from_events(&raw, &[], Some(&active_source()));
    assert_eq!((snapshot.tasks.len(), snapshot.unassigned_events), (0, 1));
}

#[test]
fn incoherent_activity_or_unknown_poll_worker_cannot_extend_a_start() {
    let raw = events(vec![start(1, 1, 1, (1, 1)), operation(1, 2, 2, EventKind::MutexAccess, 7)]);
    let mut source = active_source();
    source.runtimes[0].tasks[0].activity.as_mut().unwrap().state = TaskActivityState::Unknown;
    let unknown = TaskEventsSnapshot::from_events(&raw, &[], Some(&source));
    source.runtimes[0].tasks[0].activity.as_mut().unwrap().state = TaskActivityState::Running;
    source.runtimes[0].tasks[0].activity.as_mut().unwrap().poll_worker_id = None;
    let worker = TaskEventsSnapshot::from_events(&raw, &[], Some(&source));
    assert_eq!(
        (
            unknown.tasks.len(),
            unknown.unassigned_events,
            worker.tasks.len(),
            worker.unassigned_events
        ),
        (0, 1, 0, 1)
    );
}

#[test]
fn allocation_history_keeps_origin_free_actor_and_outside_work_distinct() {
    let snapshot = summarize(vec![
        start(1, 1, 1, (1, 1)),
        allocation(1, 2, 2, EventKind::Allocation, 7),
        finish(1, 3, 3, (1, 1), 2),
        start(2, 1, 4, (2, 1)),
        allocation(2, 2, 5, EventKind::Deallocation, 7),
        operation(2, 3, 6, EventKind::MutexAccess, 7),
        finish(2, 4, 7, (2, 1), 3),
        operation(2, 5, 8, EventKind::MutexRelease, 7),
    ]);
    let allocated = &snapshot.tasks[&(1, 1)].operations[0].objects[0];
    let freed = &snapshot.tasks[&(2, 1)].operations[0].objects[0];
    assert_eq!(
        (
            allocated.events,
            allocated.history.len(),
            Arc::ptr_eq(&allocated.history, &freed.history),
            snapshot.tasks[&(2, 1)].inferred_events,
            snapshot.histories.len(),
            allocated.history[0].detail.contains("requested=64 B align=16")
        ),
        (1, 2, true, 2, 2, true),
    );
}

#[test]
fn unassigned_free_is_related_evidence_not_work_by_the_allocating_task() {
    let snapshot = summarize(vec![
        start(1, 1, 1, (1, 1)),
        allocation(1, 2, 2, EventKind::Allocation, 7),
        finish(1, 3, 3, (1, 1), 2),
        allocation(2, 1, 4, EventKind::Deallocation, 7),
    ]);
    let task = &snapshot.tasks[&(1, 1)];
    assert_eq!(
        (
            task.inferred_events,
            task.operations[0].objects[0].history[1].task,
            snapshot.unassigned_events
        ),
        (1, None, 1)
    );
}

#[test]
fn retained_arc_lifetimes_do_not_link_recycled_addresses() {
    let snapshot = summarize(vec![
        start(1, 1, 1, (1, 1)),
        operation(1, 2, 2, EventKind::ArcCreate, 7),
        operation(1, 3, 3, EventKind::ArcDrop, 7),
        finish(1, 4, 4, (1, 1), 3),
        start(2, 1, 5, (1, 2)),
        operation(2, 2, 6, EventKind::ArcCreate, 7),
        operation(2, 3, 7, EventKind::ArcClone, 7),
        finish(2, 4, 8, (1, 2), 3),
    ]);
    let one = &snapshot.tasks[&(1, 1)].operations[0].objects[0];
    let two = &snapshot.tasks[&(1, 2)].operations[0].objects[0];
    assert_eq!(
        (one.history.len(), two.history.len(), Arc::ptr_eq(&one.history, &two.history)),
        (2, 2, false)
    );
}

#[test]
fn unordered_arc_lifecycle_ties_do_not_invent_cross_thread_lifetime_links() {
    let snapshot = summarize(vec![
        operation(1, 1, 5, EventKind::ArcCreate, 7),
        operation(2, 1, 5, EventKind::ArcClone, 7),
        operation(1, 2, 5, EventKind::ArcDrop, 7),
    ]);
    assert_eq!(
        snapshot.histories.iter().map(|history| history.events.len()).collect::<Vec<_>>(),
        vec![1, 1, 1]
    );
}

#[test]
fn reused_allocation_addresses_with_distinct_ids_are_separate_histories() {
    let snapshot = summarize(vec![
        allocation(1, 1, 1, EventKind::Allocation, 7),
        allocation(2, 1, 2, EventKind::Deallocation, 7),
        allocation(1, 2, 3, EventKind::Allocation, 8),
        allocation(2, 2, 4, EventKind::Deallocation, 8),
    ]);
    assert_eq!(
        snapshot
            .histories
            .iter()
            .map(|history| (history.object_id, history.events.len()))
            .collect::<Vec<_>>(),
        vec![(7, 2), (8, 2)],
    );
}

#[test]
fn all_existing_thread_operations_have_task_history_and_measurements() {
    let mut raw = vec![start(1, 1, 1, (1, 1))];
    for (index, kind) in ThreadOperationKind::ALL.into_iter().enumerate() {
        let sequence = u64::try_from(index).unwrap() + 2;
        let mut event = if kind.is_allocation() {
            allocation(1, sequence, sequence, kind.event_kind(), 1)
        } else {
            operation(1, sequence, sequence, kind.event_kind(), sequence)
        };
        if kind == ThreadOperationKind::ChannelHighWatermark {
            event.payload = EventPayload::Numeric(NumericEvent {
                object_id: ObjectId::new(sequence),
                value: 32,
            });
        }
        raw.push(event);
    }
    raw.push(finish(1, 36, 36, (1, 1), 35));
    let snapshot = summarize(raw);
    let task = &snapshot.tasks[&(1, 1)];
    assert_eq!(
        (
            task.inferred_events,
            task.operations.iter().map(|operation| operation.kind).collect::<Vec<_>>(),
            task.operations[30].objects[0].history[0].detail.clone()
        ),
        (34, ThreadOperationKind::ALL.to_vec(), "ChannelHighWatermark; value=32".into()),
    );
}

fn with_stack(mut event: Event, addresses: &[u64]) -> Event {
    event.call_stack = addresses.iter().copied().map(Address::new).collect();
    event
}

fn lookup(address: u64, symbol: &str) -> AddressLookup {
    AddressLookup::from_fields(AddressLookupFields {
        address,
        symbol: Some(symbol.into()),
        filename: None,
        line: None,
        column: None,
    })
}

#[test]
fn relative_stacks_follow_each_poll_site_and_preserve_nested_user_frames() {
    let raw = events(vec![
        with_stack(start(1, 1, 1, (1, 1)), &[11, 30, 40]),
        with_stack(operation(1, 2, 2, EventKind::MutexAccess, 7), &[1, 2, 30, 40]),
        with_stack(finish(1, 3, 3, (1, 1), 2), &[12, 30, 40]),
        with_stack(start(2, 1, 4, (1, 1)), &[21, 50, 60]),
        with_stack(operation(2, 2, 5, EventKind::MutexRelease, 7), &[3, 4, 50, 60]),
        with_stack(finish(2, 3, 6, (1, 1), 2), &[22, 50, 60]),
    ]);
    let addresses = [
        lookup(1, "app::inner_future"),
        lookup(2, "app::outer_future"),
        lookup(3, "app::resumed_inner"),
        lookup(4, "app::resumed_outer"),
        lookup(30, "executor::poll_a"),
        lookup(40, "executor::thread_a"),
        lookup(50, "executor::poll_b"),
        lookup(60, "executor::thread_b"),
    ];
    let snapshot = TaskEventsSnapshot::from_events(&raw, &addresses, None);
    let history = &snapshot.tasks[&(1, 1)].operations[0].objects[0].history;
    assert_eq!(
        history
            .iter()
            .map(|event| (
                event.relative_stack_known,
                event.stack(AllocationStackFilter::Application).len(),
                event.stack(AllocationStackFilter::All).len(),
                event.stack(AllocationStackFilter::Application).join("\n").contains("outer")
            ))
            .collect::<Vec<_>>(),
        vec![(true, 2, 4, true), (true, 2, 4, true)],
    );
}

#[test]
fn raw_common_boundary_addresses_work_without_symbols() {
    let raw = events(vec![
        with_stack(start(1, 1, 1, (1, 1)), &[11, 30, 40]),
        with_stack(operation(1, 2, 2, EventKind::ArcClone, 7), &[1, 2, 30, 40]),
        with_stack(finish(1, 3, 3, (1, 1), 2), &[12, 30, 40]),
    ]);
    let snapshot = TaskEventsSnapshot::from_events(&raw, &[], None);
    let event = &snapshot.histories[0].events[0];
    assert_eq!(
        (
            event.relative_stack_known,
            event.stack(AllocationStackFilter::Application).len(),
            event.stack(AllocationStackFilter::All).len()
        ),
        (true, 2, 4)
    );
}

#[test]
fn absent_task_identity_does_not_create_an_inferred_task() {
    let raw = events(vec![
        start(1, 1, 1, (1, 0)),
        operation(1, 2, 2, EventKind::MutexAccess, 7),
        finish(1, 3, 3, (1, 0), 2),
    ]);
    let snapshot = TaskEventsSnapshot::from_events(&raw, &[], None);
    assert_eq!((snapshot.tasks.len(), snapshot.unassigned_events), (0, 1));
}

#[test]
fn instrumented_future_frames_supply_a_root_without_poll_backtraces() {
    let raw = events(vec![
        start(1, 1, 1, (1, 1)),
        with_stack(operation(1, 2, 2, EventKind::MutexAccess, 7), &[1, 2, 3, 4]),
        finish(1, 3, 3, (1, 1), 2),
    ]);
    let observed = [
        "<oxidizer_rt::seismograph::TaskTelemetryFuture<app::Task> as core::future::future::Future>::poll::{{closure}}",
        "<oxidizer_rt::tasks::remote_task_future::RemoteTaskFuture<app::Task, ()> as core::future::Future>::poll",
    ]
    .map(|root| {
        let addresses = [
            lookup(1, "app::inner_future"),
            lookup(2, "app::outer_future"),
            lookup(3, root),
            lookup(4, "oxidizer_executor::ExecutorCore::poll"),
        ];
        let snapshot = TaskEventsSnapshot::from_events(&raw, &addresses, None);
        let event = &snapshot.histories[0].events[0];
        (
            event.relative_stack_known,
            event.stack(AllocationStackFilter::Application).len(),
            event.stack(AllocationStackFilter::All).len(),
        )
    });
    assert_eq!(observed, [(true, 2, 4), (true, 2, 4)]);
}

#[test]
fn wrapper_names_inside_user_type_arguments_do_not_establish_a_root() {
    let raw = events(vec![
        start(1, 1, 1, (1, 1)),
        with_stack(operation(1, 2, 2, EventKind::MutexAccess, 7), &[1, 2]),
        finish(1, 3, 3, (1, 1), 2),
    ]);
    let addresses = [
        lookup(1, "app::work"),
        lookup(2, "app::consume<oxidizer_rt::seismograph::TaskTelemetryFuture<app::Task>>::poll"),
    ];
    let snapshot = TaskEventsSnapshot::from_events(&raw, &addresses, None);
    let event = &snapshot.histories[0].events[0];
    assert_eq!(
        (event.relative_stack_known, event.stack(AllocationStackFilter::Application).len()),
        (false, 2)
    );
}

#[test]
fn runtime_symbols_do_not_attribute_events_without_poll_evidence() {
    let raw = events(vec![with_stack(operation(1, 1, 1, EventKind::MutexAccess, 7), &[1, 2])]);
    let addresses = [
        lookup(1, "app::work"),
        lookup(2, "oxidizer_rt::seismograph::TaskTelemetryFuture<app::Task>::poll"),
    ];
    let snapshot = TaskEventsSnapshot::from_events(&raw, &addresses, None);
    let event = &snapshot.histories[0].events[0];
    assert_eq!((event.task, event.relative_stack_known), (None, false));
}

#[test]
fn missing_poll_stack_is_explicitly_untrimmed_and_never_uses_spawn_stack() {
    let raw = events(vec![
        start(1, 1, 1, (1, 1)),
        with_stack(operation(1, 2, 2, EventKind::ArcClone, 7), &[1, 900]),
        finish(1, 3, 3, (1, 1), 2),
    ]);
    let snapshot = TaskEventsSnapshot::from_events(&raw, &[lookup(1, "app::work"), lookup(900, "app::spawn")], Some(&active_source()));
    let event = &snapshot.histories[0].events[0];
    assert_eq!(
        (
            event.relative_stack_known,
            event.stack(AllocationStackFilter::Application).len(),
            event.stack(AllocationStackFilter::All).len()
        ),
        (false, 2, 2)
    );
}

#[test]
fn identical_event_stacks_and_object_histories_are_shared() {
    let raw = events(vec![
        with_stack(start(1, 1, 1, (1, 1)), &[11, 30, 40]),
        with_stack(operation(1, 2, 2, EventKind::MutexAccess, 7), &[1, 30, 40]),
        with_stack(operation(1, 3, 3, EventKind::MutexRelease, 7), &[1, 30, 40]),
        with_stack(finish(1, 4, 4, (1, 1), 3), &[12, 30, 40]),
    ]);
    let snapshot = TaskEventsSnapshot::from_events(&raw, &[], None);
    let operations = &snapshot.tasks[&(1, 1)].operations;
    let history = &operations[0].objects[0].history;
    assert_eq!(
        (
            Arc::ptr_eq(history, &operations[1].objects[0].history),
            Arc::ptr_eq(&history[0].frames, &history[1].frames)
        ),
        (true, true)
    );
}

#[test]
fn many_tasks_share_one_related_history_instead_of_cloning_it_per_task() {
    let mut raw = Vec::new();
    for task in 1..=1_024 {
        let sequence = (task - 1) * 3 + 1;
        raw.extend([
            start(1, sequence, sequence, (1, task)),
            operation(1, sequence + 1, sequence + 1, EventKind::MutexAccess, 7),
            finish(1, sequence + 2, sequence + 2, (1, task), 2),
        ]);
    }
    let snapshot = summarize(raw);
    let history = &snapshot.tasks[&(1, 1)].operations[0].objects[0].history;
    assert_eq!(
        (
            snapshot.histories.len(),
            history.len(),
            snapshot
                .tasks
                .values()
                .all(|task| task.inferred_events == 1 && Arc::ptr_eq(history, &task.operations[0].objects[0].history)),
        ),
        (1, 1_024, true),
    );
}

#[test]
fn stack_filtering_keeps_original_poll_evidence_and_related_history_semantics() {
    let raw = events(vec![
        with_stack(start(1, 1, 1, (1, 1)), &[2]),
        with_stack(operation(1, 2, 2, EventKind::MutexAccess, 7), &[1]),
        with_stack(finish(1, 3, 3, (1, 1), 2), &[2]),
        with_stack(start(2, 1, 4, (1, 2)), &[2]),
        with_stack(operation(2, 2, 5, EventKind::MutexRelease, 7), &[2]),
        with_stack(finish(2, 3, 6, (1, 2), 2), &[2]),
    ]);
    let index = Arc::new(FilterIndex::new(
        DecodedSnapshot {
            events: raw,
            ..DecodedSnapshot::default()
        },
        None,
        None,
        vec![lookup(1, "app::work"), lookup(2, "noise::executor")],
        HashSet::new(),
    ));
    let filtered = index.render(&FilterSpec::parse("crate:app", "", false, RuntimeStackMode::Event).unwrap());
    let restored = index.render(&FilterSpec::default());
    assert_eq!(
        (
            actors(&filtered.task_events),
            filtered.task_events.tasks[&(1, 1)].operations[0].objects[0].history.len(),
            restored.task_events.tasks[&(1, 1)].operations[0].objects[0].history.len()
        ),
        (vec![(1, 2, Some((1, 1)), false)], 1, 2),
    );
}

#[test]
fn filtering_out_nested_boundaries_never_reattributes_ambiguous_work() {
    let raw = events(vec![
        with_stack(start(1, 1, 1, (1, 1)), &[1]),
        with_stack(start(1, 2, 2, (1, 2)), &[2]),
        with_stack(operation(1, 3, 3, EventKind::MutexAccess, 7), &[1]),
        with_stack(finish(1, 4, 4, (1, 2), 2), &[2]),
        with_stack(finish(1, 5, 5, (1, 1), 4), &[1]),
    ]);
    let index = Arc::new(FilterIndex::new(
        DecodedSnapshot {
            events: raw,
            ..DecodedSnapshot::default()
        },
        None,
        None,
        vec![lookup(1, "app::work"), lookup(2, "noise::nested")],
        HashSet::new(),
    ));
    let filtered = index.render(&FilterSpec::parse("crate:app", "", false, RuntimeStackMode::Event).unwrap());
    assert_eq!((filtered.task_events.tasks.len(), filtered.task_events.ambiguous_events), (0, 1));
}

#[test]
fn filtered_inferred_actors_remain_navigable_without_restoring_hidden_runtime_events() {
    let raw = events(vec![
        with_stack(start(1, 1, 1, (1, 1)), &[2]),
        with_stack(allocation(1, 2, 2, EventKind::Allocation, 7), &[1]),
        with_stack(finish(1, 3, 3, (1, 1), 2), &[2]),
        with_stack(start(2, 1, 4, (2, 1)), &[2]),
        with_stack(operation(2, 2, 5, EventKind::MutexAccess, 7), &[1]),
        with_stack(finish(2, 3, 6, (2, 1), 2), &[2]),
    ]);
    let index = Arc::new(FilterIndex::new(
        DecodedSnapshot {
            events: raw,
            ..DecodedSnapshot::default()
        },
        None,
        None,
        vec![lookup(1, "app::work"), lookup(2, "noise::executor")],
        HashSet::new(),
    ));
    let filtered = index.render(&FilterSpec::parse("crate:app", "", false, RuntimeStackMode::Event).unwrap());
    assert_eq!(
        (
            filtered.runtime.runtime_events,
            filtered.filter_summary.events.shown,
            filtered
                .runtime
                .workers
                .iter()
                .map(|worker| (
                    worker.runtime_id,
                    worker.worker_id,
                    worker.observed_tasks,
                    worker.metrics.poll_count,
                    worker
                        .tasks
                        .iter()
                        .map(|task| (
                            task.runtime_id,
                            task.task_id,
                            task.metrics.poll_count,
                            task.spawn_stack.len(),
                            task.worker_ids.len(),
                        ))
                        .collect::<Vec<_>>(),
                ))
                .collect::<Vec<_>>(),
        ),
        (
            0,
            2,
            vec![(1, None, 0, 0, vec![(1, 1, 0, 0, 0)]), (2, None, 0, 0, vec![(2, 1, 0, 0, 0)])]
        ),
    );
}

#[test]
fn visible_runtime_rows_are_not_duplicated_by_inferred_actor_navigation() {
    let raw = events(vec![
        with_stack(start(1, 1, 1, (1, 1)), &[2]),
        with_stack(operation(1, 2, 2, EventKind::MutexAccess, 7), &[1]),
        with_stack(finish(1, 3, 3, (1, 1), 2), &[1]),
    ]);
    let index = Arc::new(FilterIndex::new(
        DecodedSnapshot {
            events: raw,
            ..DecodedSnapshot::default()
        },
        None,
        None,
        vec![lookup(1, "app::work"), lookup(2, "noise::executor")],
        HashSet::new(),
    ));
    let filtered = index.render(&FilterSpec::parse("crate:app", "", false, RuntimeStackMode::Event).unwrap());
    assert_eq!(
        filtered
            .runtime
            .workers
            .iter()
            .map(|worker| (worker.worker_id, worker.tasks.len(), worker.metrics.poll_count,))
            .collect::<Vec<_>>(),
        vec![(Some(101), 1, 1)],
    );
}

#[test]
fn offline_preparation_carries_task_event_attribution() {
    let snapshot = super::super::snapshot::prepare(DecodedSnapshot {
        events: events(vec![
            start(1, 1, 1, (1, 1)),
            allocation(1, 2, 2, EventKind::Allocation, 7),
            finish(1, 3, 3, (1, 1), 2),
        ]),
        ..DecodedSnapshot::default()
    })
    .unwrap();
    assert_eq!(snapshot.task_events.tasks[&(1, 1)].inferred_events, 1);
}
