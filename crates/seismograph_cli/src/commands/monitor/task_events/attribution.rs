// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use seismograph::recorder::event::{Address, Event, EventClock, EventKind, Events};
use seismograph_runtime::snapshot::{Snapshot, TaskActivityState};

use super::TaskKey;

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Evidence {
    pub(super) task: Option<TaskKey>,
    pub(super) ambiguous: bool,
    pub(super) boundary: Option<Boundary>,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Boundary {
    start: Option<usize>,
    finish: Option<usize>,
}

impl Boundary {
    pub(super) fn stack(self, events: &[Event]) -> &[Address] {
        match (self.start, self.finish) {
            (Some(start), Some(finish)) => super::stacks::common_suffix(&events[start].call_stack, &events[finish].call_stack),
            (Some(boundary), None) | (None, Some(boundary)) => &events[boundary].call_stack,
            (None, None) => &[],
        }
    }
}

#[derive(Debug)]
struct Poll {
    task: TaskKey,
    thread: u64,
    /// Local sequence-ordered positions, excluding the poll boundaries themselves.
    first: usize,
    end: usize,
    started_at: u64,
    finished_at: u64,
    valid: bool,
    boundary: Boundary,
}

struct Thread<'a> {
    id: u64,
    events: &'a [Event],
    indices: Vec<usize>,
    /// Prefix count of missing/duplicate sequences or reversed timestamps.
    breaks: Vec<usize>,
    ordered_time: bool,
}

impl Thread<'_> {
    fn event(&self, position: usize) -> &Event {
        &self.events[self.indices[position]]
    }

    fn continuous(&self, first: usize, last: usize) -> bool {
        first <= last && self.breaks[first] == self.breaks[last]
    }
}

pub(super) fn infer(events: &Events, source: Option<&Snapshot>) -> Vec<Evidence> {
    let mut threads = BTreeMap::<u64, Vec<usize>>::new();
    for (index, event) in events.events.iter().enumerate() {
        threads.entry(event.thread_id.get()).or_default().push(index);
    }
    let threads = threads
        .into_iter()
        .map(|(id, mut indices)| {
            indices.sort_unstable_by_key(|index| events.events[*index].sequence);
            let mut breaks = vec![0; indices.len()];
            let mut ordered_time = true;
            for position in 1..indices.len() {
                let previous = &events.events[indices[position - 1]];
                let current = &events.events[indices[position]];
                let reversed = previous.timestamp > current.timestamp;
                if reversed {
                    ordered_time = false;
                }
                breaks[position] =
                    breaks[position - 1] + usize::from(previous.sequence.get().checked_add(1) != Some(current.sequence.get()) || reversed);
            }
            Thread {
                id,
                events: &events.events,
                indices,
                breaks,
                ordered_time,
            }
        })
        .collect::<Vec<_>>();
    // Sampling happens before sequence assignment. Continuous sequences alone
    // cannot rule out an entirely unsampled nested task.
    let complete_policy = events.clock == EventClock::ProcessMonotonic
        && events.recording.runtime_tasks.enabled
        && events.recording.runtime_tasks.event_sampling.get() == 1;
    let observations = open_observations(source);
    let totals = events
        .threads
        .iter()
        .map(|thread| (thread.thread_id.get(), thread.total_events))
        .collect::<HashMap<_, _>>();
    let mut polls = Vec::new();
    for thread in &threads {
        collect_polls(thread, complete_policy, &observations, &totals, &mut polls);
    }
    reject_concurrent_task_polls(&mut polls);
    let mut by_thread = BTreeMap::<u64, Vec<usize>>::new();
    for (index, poll) in polls.iter().enumerate() {
        by_thread.entry(poll.thread).or_default().push(index);
    }
    let mut evidence = vec![Evidence::default(); events.events.len()];
    for thread in threads {
        let Some(mut candidates) = by_thread.remove(&thread.id) else {
            continue;
        };
        candidates.sort_unstable_by_key(|index| polls[*index].first);
        assign(&thread, &polls, &candidates, &mut evidence);
    }
    evidence
}

type ObservationKey = (TaskKey, u64, u64);

fn open_observations(source: Option<&Snapshot>) -> HashMap<ObservationKey, u64> {
    source
        .into_iter()
        .flat_map(|source| &source.runtimes)
        .flat_map(|runtime| {
            runtime.tasks.iter().filter_map(|task| {
                let activity = task.activity?;
                if activity.state != TaskActivityState::Running {
                    return None;
                }
                if activity.queued_since.is_some() {
                    return None;
                }
                if activity.ready_since.is_some_and(|ready| ready > activity.observed_at) {
                    return None;
                }
                Some((
                    (
                        (runtime.id.get(), task.id.get()),
                        activity.poll_started_at?.ticks(),
                        activity.poll_worker_id?.get(),
                    ),
                    activity.observed_at.ticks(),
                ))
            })
        })
        .collect()
}

fn collect_polls(
    thread: &Thread<'_>,
    complete_policy: bool,
    observations: &HashMap<ObservationKey, u64>,
    totals: &HashMap<u64, u64>,
    polls: &mut Vec<Poll>,
) {
    let mut starts = HashMap::<TaskKey, Vec<usize>>::new();
    let first_poll = polls.len();
    for position in 0..thread.indices.len() {
        let event = thread.event(position);
        let Some(runtime) = event.runtime() else {
            continue;
        };
        if runtime.subject_id == 0 {
            continue;
        }
        let key = (runtime.runtime_id.get(), runtime.subject_id);
        match event.kind {
            EventKind::TaskPollStarted => starts.entry(key).or_default().push(position),
            EventKind::TaskPollFinished => {
                let start = starts.get_mut(&key).and_then(Vec::pop);
                if let Some(poll) = closed_poll(thread, key, start, position, complete_policy) {
                    polls.push(poll);
                }
            }
            // In particular, TaskReady belongs to the notifier and is not a
            // change of the currently executing task on this recorder thread.
            _ => {}
        }
    }
    let mut unmatched = starts.into_values().flatten().collect::<Vec<_>>();
    unmatched.sort_unstable();
    for &start in &unmatched {
        if let Some(poll) = open_poll(thread, start, complete_policy, observations, totals) {
            polls.push(poll);
        }
    }
    for poll in &mut polls[first_poll..] {
        // An unmatched nested start is not permission to treat its later work as
        // work by the enclosing task, even if that outer poll has both boundaries.
        if unmatched.iter().any(|start| (poll.first..poll.end).contains(start)) {
            poll.valid = false;
        }
    }
    reject_crossing_polls(&mut polls[first_poll..]);
}

fn reject_crossing_polls(polls: &mut [Poll]) {
    let mut indices = (0..polls.len()).collect::<Vec<_>>();
    indices.sort_unstable_by_key(|index| (polls[*index].first, std::cmp::Reverse(polls[*index].end)));
    let mut first = 0;
    while first < indices.len() {
        let mut end = first;
        let mut maximum = polls[indices[first]].end;
        let mut active = BTreeSet::new();
        let mut crossing = false;
        while
        // #[gamma::skip(cond.negate, relational.lt_to_gt, tag = "timeout", reason = "November 4 campaign: this mutation exceeded the test timeout")]
        end < indices.len()
            && (
                // #[gamma::skip(logical.or_remove_left, tag = "timeout", reason = "November 4 campaign: this mutation exceeded the test timeout")]
                end == first || polls[indices[end]].first < maximum
            )
        {
            let poll = &polls[indices[end]];
            while
            // #[gamma::skip(cond.negate, tag = "timeout", reason = "November 4 campaign: this mutation exceeded the test timeout")]
            active.first().is_some_and(|(finish, _)| *finish <= poll.first) {
                active.pop_first();
            }
            crossing |= active.first().is_some_and(|(finish, _)| *finish < poll.end);
            active.insert((poll.end, indices[end]));
            maximum = maximum.max(poll.end);
            // #[gamma::skip(stmt.delete_assign, literal.int_decrement, tag = "timeout", reason = "November 6 campaign 4d16e3a285c1 and 426dae0c4f59: disabling this cursor advance exceeded the timeout")]
            end += 1;
        }
        if crossing {
            for &index in &indices[first..end] {
                polls[index].valid = false;
            }
        }
        // #[gamma::skip(stmt.delete_assign, assign_value.default, tag = "timeout", reason = "November 6 campaign 754d53826b54 and b0bbc1ec39f4: disabling this group advance exceeded the timeout")]
        first = end;
    }
}

fn closed_poll(thread: &Thread<'_>, task: TaskKey, start: Option<usize>, finish: usize, complete_policy: bool) -> Option<Poll> {
    let event = thread.event(finish);
    let runtime = event.runtime()?;
    let recovered_start = event.timestamp.ticks().checked_sub(runtime.value_0);
    let (first, started_at, valid) = if let Some(start) = start {
        let start_event = thread.event(start);
        (
            start.saturating_add(1),
            start_event.timestamp.ticks(),
            recovered_start == Some(start_event.timestamp.ticks())
                && start_event.runtime()?.worker_id == runtime.worker_id
                && thread.continuous(start, finish),
        )
    } else {
        // Recover only the retained continuous suffix. Without a start sequence,
        // events tied with its timestamp cannot be placed on either side.
        let started_at = recovered_start?;
        if !thread.ordered_time {
            return None;
        }
        let first = thread.indices[..finish].partition_point(|index| thread.events[*index].timestamp.ticks() <= started_at);
        (first, started_at, thread.continuous(first, finish))
    };
    Some(Poll {
        task,
        thread: thread.id,
        first,
        end: finish,
        started_at,
        finished_at: event.timestamp.ticks(),
        valid: valid && complete_policy,
        boundary: Boundary {
            start: start.map(|position| thread.indices[position]),
            finish: Some(thread.indices[finish]),
        },
    })
}

fn open_poll(
    thread: &Thread<'_>,
    start: usize,
    complete_policy: bool,
    observations: &HashMap<ObservationKey, u64>,
    totals: &HashMap<u64, u64>,
) -> Option<Poll> {
    let event = thread.event(start);
    let runtime = event.runtime()?;
    let task = (runtime.runtime_id.get(), runtime.subject_id);
    let observed = *observations.get(&(task, event.timestamp.ticks(), runtime.worker_id?.get()))?;
    // Independently sampled worker.thread_id/current_task slots are never used:
    // only the retained start establishes the actual recorder thread.
    if !thread.ordered_time {
        return None;
    }
    if observed <= event.timestamp.ticks() {
        return None;
    }
    let end = thread
        .indices
        .partition_point(|index| thread.events[*index].timestamp.ticks() < observed);
    let last = thread.indices.last().map(|index| thread.events[*index].sequence.get())?;
    debug_assert!(end > start, "ordered timestamps before observed include the retained start");
    if totals.get(&thread.id) != Some(&last) {
        return None;
    }
    Some(Poll {
        task,
        thread: thread.id,
        first: start.saturating_add(1),
        end,
        started_at: event.timestamp.ticks(),
        finished_at: observed,
        valid: if complete_policy {
            thread.continuous(start, end - 1)
        } else {
            false
        },
        boundary: Boundary {
            start: Some(thread.indices[start]),
            finish: None,
        },
    })
}

fn reject_concurrent_task_polls(polls: &mut [Poll]) {
    let mut tasks = BTreeMap::<TaskKey, Vec<usize>>::new();
    for (index, poll) in polls.iter().enumerate() {
        tasks.entry(poll.task).or_default().push(index);
    }

    for mut indices in tasks.into_values() {
        indices.sort_unstable_by_key(|index| polls[*index].started_at);
        let mut first = 0;
        while first < indices.len() {
            let thread = polls[indices[first]].thread;
            let mut end_time = polls[indices[first]].finished_at;
            let mut end = first;
            let mut multiple_threads = false;
            loop {
                let next = &polls[indices[end]];
                multiple_threads |= next.thread != thread;
                end_time = end_time.max(next.finished_at);
                end += 1;
                if end == indices.len() || polls[indices[end]].started_at > end_time {
                    break;
                }
            }
            if multiple_threads {
                for &index in &indices[first..end] {
                    polls[index].valid = false;
                }
            }
            first = end;
        }
    }
}

fn assign(thread: &Thread<'_>, polls: &[Poll], candidates: &[usize], evidence: &mut [Evidence]) {
    let mut active = BTreeSet::new();
    let mut endings = BTreeSet::new();
    let mut next = 0;
    for (position, &index) in thread.indices.iter().enumerate() {
        while let Some(&(end, candidate)) = endings.first() {
            if end > position {
                // #[gamma::skip(loop.break_to_continue, tag = "timeout", reason = "November 4 campaign: this mutation exceeded the test timeout")]
                break;
            }

            // #[gamma::skip(stmt.delete_call, tag = "timeout", reason = "November 4 campaign: this mutation exceeded the test timeout")]
            endings.remove(&(end, candidate));
            active.remove(&candidate);
        }
        while let Some(&candidate) = candidates.get(next) {
            let poll = &polls[candidate];
            if poll.first > position {
                // #[gamma::skip(loop.break_to_continue, tag = "timeout", reason = "November 4 campaign: this mutation exceeded the test timeout")]
                break;
            }
            if poll.end > position {
                active.insert(candidate);
                endings.insert((poll.end, candidate));
            }
            // #[gamma::skip(stmt.delete_assign, tag = "timeout", reason = "November 4 campaign: this mutation exceeded the test timeout")]
            next +=
            // #[gamma::skip(literal.int_decrement, tag = "timeout", reason = "November 4 campaign: this mutation exceeded the test timeout")]
            1;
        }
        let Some(&candidate) = active.first() else {
            continue;
        };
        let poll = &polls[candidate];
        evidence[index] = if active.len() == 1 && poll.valid {
            Evidence {
                task: Some(poll.task),
                ambiguous: false,
                boundary: Some(poll.boundary),
            }
        } else {
            Evidence {
                ambiguous: true,
                ..Evidence::default()
            }
        };
    }
}

#[cfg(test)]
mod tests {
    use seismograph::recorder::event::{EventPayload, EventSequence, EventTimestamp};
    use seismograph::recorder::runtime::{RuntimeEvent, RuntimeId, WorkerId};
    use seismograph::recorder::thread::ThreadId;

    use super::*;

    fn runtime_event(kind: EventKind, timestamp: u64, duration: u64) -> Event {
        Event {
            thread_id: ThreadId::new(1),
            sequence: EventSequence::new(1),
            timestamp: EventTimestamp::from_ticks(timestamp),
            kind,
            payload: EventPayload::Runtime(RuntimeEvent {
                runtime_id: RuntimeId::from_raw(1).unwrap(),
                worker_id: WorkerId::from_raw(1),
                subject_id: 1,
                related_id: 0,
                value_0: duration,
                value_1: 0,
            }),
            call_stack: Vec::new(),
        }
    }

    fn poll(first: usize, end: usize) -> Poll {
        Poll {
            task: (1, 1),
            thread: 1,
            first,
            end,
            started_at: u64::try_from(first).unwrap(),
            finished_at: u64::try_from(end).unwrap(),
            valid: true,
            boundary: Boundary { start: None, finish: None },
        }
    }

    #[test]
    fn gamma_survivor_attribution_open_poll_uses_actual_start_and_continuity() {
        let mut events = [
            runtime_event(EventKind::TaskPollFinished, 10, 1),
            runtime_event(EventKind::TaskPollStarted, 20, 0),
            runtime_event(EventKind::TaskPollStarted, 25, 0),
        ];
        for (index, event) in events.iter_mut().enumerate() {
            event.sequence = EventSequence::new(u64::try_from(index + 1).unwrap());
        }
        let thread = Thread {
            id: 1,
            events: &events,
            indices: vec![0, 1, 2],
            breaks: vec![0, 0, 0],
            ordered_time: true,
        };
        let observations = HashMap::from([(((1, 1), 20, 1), 30)]);
        let totals = HashMap::from([(1, 3)]);
        let result = open_poll(&thread, 1, true, &observations, &totals).unwrap();
        assert_eq!(
            (result.first, result.end, result.started_at, result.finished_at, result.valid),
            (2, 3, 20, 30, true)
        );
        assert_eq!((result.boundary.start, result.boundary.finish), (Some(1), None));
        assert!(open_poll(&thread, 1, true, &HashMap::from([(((1, 1), 20, 1), 20)]), &totals).is_none());
        let broken = Thread {
            breaks: vec![0, 0, 1],
            ..thread
        };
        assert!(!open_poll(&broken, 1, true, &observations, &totals).unwrap().valid);
        let mut ordinary = runtime_event(EventKind::TaskPollStarted, 20, 0);
        ordinary.payload = EventPayload::Object(seismograph::recorder::event::ObjectId::new(1));
        let ordinary_events = [ordinary];
        let ordinary_thread = Thread {
            id: 1,
            events: &ordinary_events,
            indices: vec![0],
            breaks: vec![0],
            ordered_time: true,
        };
        assert!(open_poll(&ordinary_thread, 0, true, &observations, &totals).is_none());
        assert!(closed_poll(&ordinary_thread, (1, 1), None, 0, true).is_none());
        let mut no_worker = runtime_event(EventKind::TaskPollStarted, 20, 0);
        if let EventPayload::Runtime(runtime) = &mut no_worker.payload {
            runtime.worker_id = None;
        }
        let events = [no_worker];
        let thread = Thread {
            id: 1,
            events: &events,
            indices: vec![0],
            breaks: vec![0],
            ordered_time: true,
        };
        assert!(open_poll(&thread, 0, true, &observations, &totals).is_none());
    }

    #[test]
    fn gamma_survivor_crossing_rejection_expires_nested_polls_and_extends_groups() {
        let mut polls = [poll(0, 10), poll(1, 3), poll(4, 6), poll(10, 12), poll(11, 13), poll(20, 21)];
        reject_crossing_polls(&mut polls);
        assert_eq!(polls.map(|poll| poll.valid), [true, true, true, false, false, true]);
        let mut polls = [poll(10, 15), poll(0, 5), poll(4, 11), poll(20, 21)];
        polls[0].thread = 2;
        reject_concurrent_task_polls(&mut polls);
        assert_eq!(polls.map(|poll| poll.valid), [false, false, false, true]);
    }

    #[test]
    fn gamma_survivor_assignment_removes_ended_polls_before_later_evidence() {
        let events = (0..8).map(|i| runtime_event(EventKind::TaskPollStarted, i, 0)).collect::<Vec<_>>();
        let thread = Thread {
            id: 1,
            events: &events,
            indices: (0..8).collect(),
            breaks: vec![0; 8],
            ordered_time: true,
        };
        let polls = [
            poll(1, 3),
            Poll {
                task: (1, 2),
                ..poll(4, 6)
            },
        ];
        let mut evidence = vec![Evidence::default(); 8];
        assign(&thread, &polls, &[0, 1], &mut evidence);
        assert_eq!(
            evidence.iter().map(|item| item.task).collect::<Vec<_>>(),
            [None, Some((1, 1)), Some((1, 1)), None, Some((1, 2)), Some((1, 2)), None, None]
        );
    }

    #[test]
    fn gamma_survivor_attribution_keeps_later_threads_and_requires_complete_clock_and_policy() {
        let mut ignored = runtime_event(EventKind::TaskPollStarted, 1, 0);
        ignored.payload = EventPayload::Object(seismograph::recorder::event::ObjectId::new(1));
        let mut start = runtime_event(EventKind::TaskPollStarted, 10, 0);
        let mut inside = runtime_event(EventKind::TaskPollStarted, 15, 0);
        inside.payload = EventPayload::Object(seismograph::recorder::event::ObjectId::new(1));
        let mut finish = runtime_event(EventKind::TaskPollFinished, 20, 10);
        for (index, event) in [&mut start, &mut inside, &mut finish].into_iter().enumerate() {
            event.thread_id = ThreadId::new(2);
            event.sequence = EventSequence::new(u64::try_from(index + 1).unwrap());
        }
        let mut events = Events {
            clock: EventClock::ProcessMonotonic,
            events: vec![ignored, start, inside, finish],
            ..Events::default()
        };
        events.recording.runtime_tasks.enabled = true;
        assert_eq!(infer(&events, None)[2].task, Some((1, 1)));
        events.recording.runtime_tasks.event_sampling = seismograph::recorder::EventSampling::one_in(2).unwrap();
        assert!(infer(&events, None)[2].task.is_none());
        events.recording.runtime_tasks.event_sampling = seismograph::recorder::EventSampling::one_in(1).unwrap();
        events.clock = EventClock::Unspecified;
        assert!(infer(&events, None)[2].task.is_none());
    }

    #[test]
    fn empty_boundaries_and_completed_nested_polls_are_handled() {
        assert!(Boundary { start: None, finish: None }.stack(&[]).is_empty());
        let mut polls = [poll(0, 10), poll(1, 2), poll(3, 4)];

        reject_crossing_polls(&mut polls);

        assert!(polls.iter().all(|poll| poll.valid));
    }

    #[test]
    fn touching_poll_groups_do_not_share_crossing_rejection() {
        let mut polls = [poll(0, 3), poll(3, 6), poll(4, 8)];

        reject_crossing_polls(&mut polls);

        assert_eq!(polls.map(|poll| poll.valid), [true, false, false]);
    }

    #[test]
    fn nested_polls_with_a_shared_end_are_not_crossing() {
        let mut polls = [poll(0, 5), poll(2, 5)];

        reject_crossing_polls(&mut polls);

        assert_eq!(polls.map(|poll| poll.valid), [true, true]);
    }

    #[test]
    fn concurrent_task_rejection_is_limited_to_overlapping_groups() {
        let mut other_thread = poll(4, 7);
        other_thread.thread = 2;
        let mut polls = [poll(0, 2), poll(3, 6), other_thread, poll(8, 10)];

        reject_concurrent_task_polls(&mut polls);

        assert_eq!(polls.map(|poll| poll.valid), [true, false, false, true]);
    }

    #[test]
    fn task_polls_touching_in_time_on_distinct_threads_are_ambiguous() {
        let mut other_thread = poll(2, 4);
        other_thread.thread = 2;
        let mut polls = [poll(0, 2), other_thread];

        reject_concurrent_task_polls(&mut polls);

        assert_eq!(polls.map(|poll| poll.valid), [false, false]);
    }

    #[test]
    fn reversed_poll_time_does_not_block_later_task_polls() {
        let mut reversed = poll(0, 2);
        reversed.started_at = 3;
        reversed.valid = false;
        let mut polls = [reversed, poll(4, 6)];

        reject_concurrent_task_polls(&mut polls);

        assert_eq!(polls.map(|poll| poll.valid), [false, true]);
    }

    #[test]
    fn empty_poll_interior_does_not_attribute_its_finish_boundary() {
        let events = [
            runtime_event(EventKind::TaskPollStarted, 100, 0),
            runtime_event(EventKind::TaskPollFinished, 101, 1),
        ];
        let thread = Thread {
            id: 1,
            events: &events,
            indices: vec![0, 1],
            breaks: vec![0, 0],
            ordered_time: true,
        };
        let poll = closed_poll(&thread, (1, 1), Some(0), 1, true).unwrap();
        let mut evidence = vec![Evidence::default(); events.len()];

        assign(&thread, &[poll], &[0], &mut evidence);

        assert_eq!(
            evidence.iter().map(|item| (item.task, item.ambiguous)).collect::<Vec<_>>(),
            [(None, false), (None, false)]
        );
    }

    #[test]
    fn recovered_and_open_polls_require_monotonic_observation_time() {
        let finish = runtime_event(EventKind::TaskPollFinished, 100, 50);
        let events = [finish];
        let reversed = Thread {
            id: 1,
            events: &events,
            indices: vec![0],
            breaks: vec![0],
            ordered_time: false,
        };
        assert!(closed_poll(&reversed, (1, 1), None, 0, true).is_none());

        let start = runtime_event(EventKind::TaskPollStarted, 100, 0);
        let events = [start];
        let thread = Thread {
            id: 1,
            events: &events,
            indices: vec![0],
            breaks: vec![0],
            ordered_time: true,
        };
        let observations = HashMap::from([(((1, 1), 100, 1), 99)]);
        let totals = HashMap::from([(1, 1)]);
        assert!(open_poll(&thread, 0, true, &observations, &totals).is_none());
    }
}
