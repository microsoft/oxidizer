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
                ordered_time &= !reversed;
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
                if activity.state != TaskActivityState::Running
                    || activity.queued_since.is_some()
                    || activity.ready_since.is_some_and(|ready| ready > activity.observed_at)
                {
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
        if unmatched.partition_point(|start| *start < poll.first) < unmatched.partition_point(|start| *start < poll.end) {
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
        while end < indices.len() && (end == first || polls[indices[end]].first < maximum) {
            let poll = &polls[indices[end]];
            while active.first().is_some_and(|(finish, _)| *finish <= poll.first) {
                active.pop_first();
            }
            crossing |= active.first().is_some_and(|(finish, _)| *finish < poll.end);
            active.insert((poll.end, indices[end]));
            maximum = maximum.max(poll.end);
            end += 1;
        }
        if crossing {
            for &index in &indices[first..end] {
                polls[index].valid = false;
            }
        }
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
            start + 1,
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
    if !thread.ordered_time || observed < event.timestamp.ticks() {
        return None;
    }
    let end = thread
        .indices
        .partition_point(|index| thread.events[*index].timestamp.ticks() < observed);
    let last = thread.indices.last().map(|index| thread.events[*index].sequence.get())?;
    if end <= start || totals.get(&thread.id) != Some(&last) {
        return None;
    }
    Some(Poll {
        task,
        thread: thread.id,
        first: start + 1,
        end,
        started_at: event.timestamp.ticks(),
        finished_at: observed,
        valid: complete_policy && thread.continuous(start, end - 1),
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
            let mut end = first + 1;
            let mut multiple_threads = false;
            while end < indices.len() && polls[indices[end]].started_at <= end_time {
                let next = &polls[indices[end]];
                multiple_threads |= next.thread != thread;
                end_time = end_time.max(next.finished_at);
                end += 1;
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
                break;
            }
            endings.remove(&(end, candidate));
            active.remove(&candidate);
        }
        while let Some(&candidate) = candidates.get(next) {
            let poll = &polls[candidate];
            if poll.first > position {
                break;
            }
            if poll.end > position {
                active.insert(candidate);
                endings.insert((poll.end, candidate));
            }
            next += 1;
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
