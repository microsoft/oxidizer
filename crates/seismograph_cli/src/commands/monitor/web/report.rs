// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Paged projections of the same snapshot analysis used by the terminal monitor.

mod allocator;
#[cfg(test)]
mod tests;

use tonic::Status;

use super::super::data::{
    AllocationSort, AllocationStackFilter, CapturedSnapshot, PrimitiveSort, RuntimeTaskSort, RuntimeTaskSummary, RuntimeWorkerSummary,
    average_bytes,
};
use super::super::runtime_timeline::{self, ExecutionMetrics, Interval, TimeWindow};
use super::api::row::Model;
use super::api::{self, Field, Page, Row};

const PAGE_SIZE: usize = 50;

macro_rules! fields {
    ($($name:literal => $value:expr),* $(,)?) => {
        vec![$(Field { name: $name.into(), value: $value.to_string() }),*]
    };
}
use fields;

pub(super) fn summary(id: &str, name: &str, snapshot: &CapturedSnapshot) -> api::Snapshot {
    let filter = snapshot.filter_summary;
    let count =
        |count: super::super::filter_index::FilterCounts| format!("{} of {} shown ({} unknown)", count.shown, count.total, count.unknown);
    api::Snapshot {
        id: id.into(),
        name: name.into(),
        sections: [
            ("overview", "Overview", true),
            ("allocations", "Allocations", snapshot.allocations.is_some()),
            ("snmalloc", "snmalloc", snapshot.native.is_some()),
            ("mimalloc", "Mimalloc", snapshot.mimalloc.is_some()),
            ("runtime", "Runtime", true),
            ("threads", "Threads", true),
            ("primitives", "Primitives", true),
            ("io", "I/O", true),
            ("cache", "Cache", true),
        ]
        .into_iter()
        .filter(|(_, _, show)| *show)
        .map(|(id, title, _)| api::Section {
            id: id.into(),
            title: title.into(),
        })
        .collect(),
        warnings: snapshot.heap_error.iter().cloned().collect(),
        filter_summary: if filter.active {
            fields!["Events" => count(filter.events), "Allocations" => count(filter.allocations), "Tasks" => count(filter.tasks)]
        } else {
            Vec::new()
        },
        ..api::Snapshot::default()
    }
}

pub(super) fn inspect(snapshot: &CapturedSnapshot, request: &api::InspectRequest) -> Result<Page, Status> {
    let offset = usize::try_from(request.offset).unwrap_or(usize::MAX);
    let stack = if request.full_stack {
        AllocationStackFilter::All
    } else {
        AllocationStackFilter::Application
    };
    if request.section == "runtime" {
        return runtime(snapshot, &request.parent, offset, stack, &request.sort_by, request.descending);
    }
    let path = indices(&request.parent)?;
    match request.section.as_str() {
        "overview" if path.is_empty() => Ok(overview(snapshot, offset)),
        "threads" => threads(snapshot, &path, offset, stack),
        "primitives" => primitives(snapshot, &path, offset, stack, &request.sort_by, request.descending),
        "allocations" => allocations(snapshot, &path, offset, stack, &request.sort_by, request.descending),
        "io" => io(snapshot, &path, offset),
        "cache" => cache(snapshot, &path, offset),
        "snmalloc" => allocator::native(snapshot, &path, offset),
        "mimalloc" => allocator::mimalloc(snapshot, &path, offset),
        _ => Err(Status::not_found("Snapshot section not found.")),
    }
}

fn indices(parent: &str) -> Result<Vec<usize>, Status> {
    if parent.is_empty() {
        return Ok(Vec::new());
    }
    if parent.len() > 80 {
        return Err(bad_path());
    }
    parent
        .split('/')
        .map(str::parse::<usize>)
        .collect::<Result<_, _>>()
        .map_err(|_invalid_index| bad_path())
}

fn row(title: impl Into<String>, fields: Vec<Field>) -> Row {
    Row {
        title: title.into(),
        fields,
        ..Row::default()
    }
}

fn typed(title: impl Into<String>, model: Model, has_children: bool, stack: &[String]) -> Row {
    Row {
        title: title.into(),
        model: Some(model),
        has_children,
        stack: stack.to_vec(),
        ..Row::default()
    }
}

fn page<T>(items: &[T], offset: usize, map: impl FnMut(&T) -> Row) -> Page {
    paginate(items.iter(), offset, map)
}

fn paginate<T>(items: impl ExactSizeIterator<Item = T>, offset: usize, map: impl FnMut(T) -> Row) -> Page {
    Page {
        total: u32::try_from(items.len()).unwrap_or(u32::MAX),
        rows: items.skip(offset).take(PAGE_SIZE).map(map).collect(),
        ..Page::default()
    }
}

fn bad_path() -> Status {
    Status::not_found("Snapshot detail not found.")
}

fn at<T>(values: &[T], index: usize) -> Result<&T, Status> {
    values.get(index).ok_or_else(bad_path)
}

fn shown<T: std::fmt::Display>(value: Option<T>) -> String {
    value.map_or_else(|| "Unknown".into(), |value| value.to_string())
}

fn overview(snapshot: &CapturedSnapshot, offset: usize) -> Page {
    let mut rows = vec![
        row(
            "Recorder",
            fields![
                "Total events" => snapshot.runtime.total_events, "Retained" => snapshot.runtime.retained_events,
                "Lost" => snapshot.runtime.lost_events, "Runtime events" => snapshot.runtime.runtime_events,
                "Runtime source" => if snapshot.runtime.source_present { "Present" } else { "Unavailable" },
            ],
        ),
        row(
            "Task attribution",
            fields![
                "Unassigned events" => snapshot.task_events.unassigned_events, "Ambiguous events" => snapshot.task_events.ambiguous_events,
            ],
        ),
    ];
    if let Some(memory) = snapshot.memory.as_ref() {
        rows.push(row(
            "Memory",
            fields![
                "Live bytes" => memory.live_bytes, "Peak live bytes" => memory.peak_live_bytes,
                "Peak scope" => format_args!("{:?}", memory.peak_live_bytes_scope),
                "Reserved bytes" => memory.reserved_bytes, "Mapped bytes" => memory.mapped_bytes,
                "Allocations" => memory.allocations,
            ],
        ));
    }
    paginate(rows.into_iter(), offset, std::convert::identity)
}

fn execution(value: &ExecutionMetrics) -> api::Execution {
    api::Execution {
        polls: value.poll_count,
        median_poll_nanos: value.median_poll_nanos,
        max_poll_nanos: value.max_poll_nanos,
        executing_fraction: value.executing_fraction,
    }
}

fn timeline(title: &str, intervals: &[Interval], window: TimeWindow) -> api::Timeline {
    api::Timeline {
        title: title.into(),
        start_nanos: window.start,
        end_nanos: window.end,
        bins: runtime_timeline::bins(intervals, Some(window), 64)
            .into_iter()
            .map(|fraction| api::Occupancy { fraction })
            .collect(),
    }
}

fn runtime_charts(page: &mut Page, metrics: &ExecutionMetrics, window: Option<TimeWindow>, task: bool) {
    if let Some(window) = window.filter(|value| value.end > value.start) {
        page.timelines
            .push(timeline("Observed polls on this worker", &metrics.polls, window));
        if task {
            page.timelines
                .push(timeline("Ready intervals across workers", &metrics.ready, window));
        }
    }
    let completed = if metrics.ready_samples.is_empty() {
        ("Wake-to-poll latency (includes overlap)", metrics.wake_samples.as_ref())
    } else {
        ("Completed ready waits", metrics.ready_samples.as_ref())
    };
    for (title, samples) in [("Completed poll durations", metrics.poll_samples.as_slice()), completed] {
        let buckets = runtime_timeline::histogram(samples, runtime_timeline::HISTOGRAM_BINS);
        if !buckets.is_empty() {
            page.histograms.push(api::Histogram {
                title: title.into(),
                buckets: buckets
                    .into_iter()
                    .map(|bucket| api::DurationBucket {
                        lower_nanos: bucket.lower,
                        upper_nanos: bucket.upper,
                        count: bucket.count,
                    })
                    .collect(),
            });
        }
    }
}

fn worker_row(worker: &RuntimeWorkerSummary) -> Row {
    let title = match worker.worker_id {
        Some(id) => format!("{} / Worker {id}", worker.runtime_name),
        None => format!("{} / {}", worker.runtime_name, worker.role),
    };
    typed(
        title,
        Model::RuntimeWorker(api::RuntimeWorker {
            runtime_id: worker.runtime_id,
            runtime_name: worker.runtime_name.clone(),
            worker_id: worker.worker_id,
            role: worker.role.clone(),
            state: worker.state.clone(),
            thread_id: worker.thread_id,
            current_task: worker.current_task,
            observed_tasks: worker.observed_tasks.try_into().unwrap_or(u64::MAX),
            execution: Some(execution(&worker.metrics)),
        }),
        true,
        &[],
    )
}

fn task_row(task: &RuntimeTaskSummary, stack: AllocationStackFilter) -> Row {
    typed(
        format!("Task {}", task.task_id),
        Model::RuntimeTask(api::RuntimeTask {
            runtime_id: task.runtime_id,
            task_id: task.task_id,
            parent_id: task.parent_id,
            future_size_bytes: task.future_size_bytes,
            state: task.state.clone(),
            worker_ids: task.worker_ids.clone(),
            execution: Some(execution(&task.metrics)),
            activity_state: task.activity.state.clone(),
            running_nanos: task.activity.running_for,
            ready_nanos: task.activity.ready_for,
            poll_worker_id: task.activity.poll_worker_id,
            repoll_requested: task.activity.repoll_requested,
            type_descriptor_id: task.type_descriptor_id,
        }),
        true,
        match stack {
            AllocationStackFilter::Application => &task.application_spawn_stack,
            AllocationStackFilter::All => &task.spawn_stack,
        },
    )
}

fn runtime(
    snapshot: &CapturedSnapshot,
    parent: &str,
    offset: usize,
    stack: AllocationStackFilter,
    sort_by: &str,
    descending: bool,
) -> Result<Page, Status> {
    let order = match sort_by {
        "" | "task" => RuntimeTaskSort::Task,
        "future" => RuntimeTaskSort::FutureSize,
        "polls" => RuntimeTaskSort::Polls,
        "executing" => RuntimeTaskSort::Executing,
        "median" => RuntimeTaskSort::MedianPoll,
        "maximum" => RuntimeTaskSort::MaximumPoll,
        _ => return Err(Status::invalid_argument("Unknown task sort.")),
    };
    let workers = &snapshot.runtime.workers;
    let tasks = &snapshot.task_events.tasks;
    if parent.is_empty() {
        return Ok(page(workers, offset, worker_row));
    }
    let (worker, task, operation) = if let Some(path) = parent.strip_prefix("r/") {
        if path.len() > 160 {
            return Err(bad_path());
        }
        let mut parts = path.splitn(4, '/');
        let runtime_id = parts.next().and_then(|id| id.parse::<u64>().ok()).ok_or_else(bad_path)?;
        let worker_id = match parts.next() {
            Some("unbound") => None,
            Some(id) => Some(id.parse::<u64>().map_err(|_invalid_id| bad_path())?),
            None => return Err(bad_path()),
        };
        let worker = workers
            .iter()
            .find(|row| row.runtime_id == runtime_id && row.worker_id == worker_id)
            .ok_or_else(|| Status::not_found("This worker is not in the snapshot."))?;
        let task = if let Some(id) = parts.next() {
            let id = id.parse::<u64>().map_err(|_invalid_id| bad_path())?;
            Some(
                worker
                    .tasks
                    .iter()
                    .find(|row| row.task_id == id)
                    .ok_or_else(|| Status::not_found("This task is not on the selected worker in this snapshot."))?,
            )
        } else {
            None
        };
        (worker, task, parts.next().map(RuntimeOperation::Label))
    } else {
        let path = indices(parent)?;
        let (&index, rest) = path.split_first().ok_or_else(bad_path)?;
        let worker = at(workers, index)?;
        let (task, operation) = match *rest {
            [] => (None, None),
            [task] => (Some(*at(&worker.sorted_tasks(order, descending), task)?), None),
            [task, operation] => (
                Some(*at(&worker.sorted_tasks(order, descending), task)?),
                Some(RuntimeOperation::Index(operation)),
            ),
            _ => return Err(bad_path()),
        };
        (worker, task, operation)
    };

    let Some(task) = task else {
        let mut result = paginate(worker.sorted_tasks(order, descending).into_iter(), offset, |task| {
            task_row(task, stack)
        });
        result.subject = Some(worker_row(worker));
        runtime_charts(&mut result, &worker.metrics, worker.window, false);
        return Ok(result);
    };
    let operations = tasks
        .get(&(task.runtime_id, task.task_id))
        .map_or(&[][..], |summary| &summary.operations);
    if let Some(operation) = operation {
        let operation = match operation {
            RuntimeOperation::Index(index) => at(operations, index)?,
            RuntimeOperation::Label(label) => operations
                .iter()
                .find(|row| row.kind.label() == label)
                .ok_or_else(|| Status::not_found("This operation is not in the snapshot."))?,
        };
        return Ok(paginate(operation.occurrences(), offset, |(object, event)| {
            typed(
                format!("{} / Object {}", operation.kind.label(), object.object_id),
                Model::TaskOccurrence(api::TaskOccurrence {
                    object_id: object.object_id,
                    thread_id: event.thread_id,
                    sequence: event.sequence,
                    timestamp_nanos: event.timestamp,
                    description: event.detail.clone(),
                    identity: object.identity_note.into(),
                    relative_stack_known: event.relative_stack_known,
                }),
                false,
                event.stack(stack),
            )
        }));
    }
    let mut result = page(operations, offset, |operation| Row {
        has_children: !operation.objects.is_empty(),
        ..row(operation.kind.label(), fields!["Inferred events" => operation.events])
    });
    result.subject = Some(task_row(task, stack));
    runtime_charts(&mut result, &task.metrics, worker.window, true);
    Ok(result)
}

enum RuntimeOperation<'a> {
    Index(usize),
    Label(&'a str),
}

fn threads(snapshot: &CapturedSnapshot, path: &[usize], offset: usize, stack: AllocationStackFilter) -> Result<Page, Status> {
    let threads = &snapshot.threads.threads;
    match *path {
        [] => Ok(page(threads, offset, |thread| {
            typed(
                if thread.name.is_empty() {
                    format!("Thread {}", thread.thread_id)
                } else {
                    thread.name.clone()
                },
                Model::Thread(api::Thread {
                    id: thread.thread_id,
                    name: thread.name.clone(),
                    total: thread.total_events,
                    retained: thread.retained_events,
                    lost: thread.lost_events,
                }),
                !thread.operations.is_empty(),
                &[],
            )
        })),
        [thread] => Ok(page(&at(threads, thread)?.operations, offset, |operation| Row {
            has_children: !operation.participants.is_empty(),
            ..row(
                operation.kind.label(),
                fields!["Events" => operation.events, "Objects" => operation.objects],
            )
        })),
        [thread, operation] => Ok(page(
            &at(&at(threads, thread)?.operations, operation)?.participants,
            offset,
            |peer| Row {
                has_children: !peer.objects.is_empty(),
                ..row(&peer.name, fields!["Thread" => peer.thread_id, "Events" => peer.events])
            },
        )),
        [thread, operation, peer] => {
            let operation = at(&at(threads, thread)?.operations, operation)?;
            Ok(page(&at(&operation.participants, peer)?.objects, offset, |object| Row {
                related_stacks: [
                    ("Selected thread stack", object.selected_stack()),
                    ("Related thread stack", object.related_stack()),
                ]
                .into_iter()
                .map(|(title, value)| api::LabeledStack {
                    title: title.into(),
                    frames: value.map_or_else(Vec::new, |value| value.stack(stack).to_vec()),
                })
                .collect(),
                ..row(
                    format!("Object {}", object.object_id),
                    fields![
                        "Selected thread events" => object.selected_events, "Related thread events" => object.related_events,
                    ],
                )
            }))
        }
        _ => Err(bad_path()),
    }
}

fn primitives(
    snapshot: &CapturedSnapshot,
    path: &[usize],
    offset: usize,
    stack: AllocationStackFilter,
    sort_by: &str,
    descending: bool,
) -> Result<Page, Status> {
    let order = match sort_by {
        "" | "events" => PrimitiveSort::Events,
        "objects" => PrimitiveSort::Objects,
        "threads" => PrimitiveSort::Threads,
        "hotspots" => PrimitiveSort::Hotspots,
        _ => return Err(Status::invalid_argument("Unknown primitive sort.")),
    };
    let groups = &snapshot.primitives.groups;
    match *path {
        [] => Ok(page(groups, offset, |group| {
            typed(
                group.kind.label(),
                Model::PrimitiveGroup(api::PrimitiveGroup {
                    kind: group.kind.label().into(),
                    events: group.events,
                    objects: group.objects,
                    contentions: group.contentions,
                }),
                !group.operations.is_empty(),
                &[],
            )
        })),
        [group] => Ok(paginate(
            at(groups, group)?.sorted_operations(order, descending).into_iter(),
            offset,
            |operation| Row {
                has_children: !operation.hotspots.is_empty(),
                ..row(
                    operation.kind.label(),
                    fields![
                        "Events" => operation.events, "Objects" => operation.objects, "Threads" => operation.threads,
                        "Hotspots" => operation.hotspots.len(),
                    ],
                )
            },
        )),
        [group, operation] => {
            let ordered = at(groups, group)?.sorted_operations(order, descending);
            Ok(page(&at(&ordered, operation)?.hotspots, offset, |hotspot| Row {
                stack: hotspot.stack(stack).to_vec(),
                ..row(hotspot.location(stack), fields!["Occurrences" => hotspot.count])
            }))
        }
        _ => Err(bad_path()),
    }
}

fn allocations(
    snapshot: &CapturedSnapshot,
    path: &[usize],
    offset: usize,
    stack: AllocationStackFilter,
    sort_by: &str,
    descending: bool,
) -> Result<Page, Status> {
    let data = snapshot.allocations.as_ref().ok_or_else(bad_path)?;
    match *path {
        [] => Ok(page(&["Hotspots", "Allocation records"], offset, |title| Row {
            has_children: true,
            ..row(*title, Vec::new())
        })),
        [0] => {
            let order = match sort_by {
                "" | "allocated" => AllocationSort::AllocatedBytes,
                "count" => AllocationSort::Allocations,
                "average" => AllocationSort::AverageBytes,
                "live" => AllocationSort::LiveBytes,
                "live-count" => AllocationSort::LiveAllocations,
                _ => return Err(Status::invalid_argument("Unknown allocation sort.")),
            };
            Ok(paginate(data.sorted_hotspots(order, descending).into_iter(), offset, |hotspot| {
                let mut row = typed(
                    hotspot.location(stack),
                    Model::AllocationHotspot(api::AllocationHotspot {
                        allocations: hotspot.allocations,
                        allocated_bytes: hotspot.allocated_bytes,
                        live_allocations: hotspot.live_allocations,
                        live_bytes: hotspot.live_bytes,
                    }),
                    false,
                    hotspot.stack(stack),
                );
                row.fields = fields!["Average allocation bytes" => average_bytes(hotspot)];
                row
            }))
        }
        [1] => Ok(page(&data.records, offset, |record| Row {
            stack: record.stack(stack).to_vec(),
            ..row(
                record.label(),
                record
                    .details()
                    .into_iter()
                    .map(|value| Field {
                        name: String::new(),
                        value,
                    })
                    .collect(),
            )
        })),
        _ => Err(bad_path()),
    }
}

fn io(snapshot: &CapturedSnapshot, path: &[usize], offset: usize) -> Result<Page, Status> {
    let resources = &snapshot.io.resources;
    match *path {
        [] => Ok(page(resources, offset, |resource| {
            typed(
                format!("{:?} {}", resource.kind, resource.resource_id),
                Model::IoResource(api::IoResource {
                    id: resource.resource_id,
                    kind: format!("{:?}", resource.kind),
                    reads: resource.reads,
                    writes: resource.writes,
                    completed: resource.completed,
                    errors: resource.errors,
                    canceled: resource.canceled,
                    requested_bytes: resource.requested_bytes,
                    completed_bytes: resource.completed_bytes,
                }),
                !resource.operations.is_empty(),
                &[],
            )
        })),
        [resource] => Ok(page(&at(resources, resource)?.operations, offset, |operation| {
            row(
                format!("{} {}", operation.kind.label(), operation.operation_id),
                fields![
                    "Outcome" => format_args!("{:?}", operation.outcome), "Thread" => operation.thread_id,
                    "Duration (ns)" => shown(operation.duration_nanos), "Requested bytes" => operation.requested_bytes,
                    "Completed bytes" => operation.completed_bytes, "Buffer" => shown(operation.buffer_id),
                    "Buffer bytes" => operation.buffer_len, "Buffer spans" => operation.buffer_span_count,
                ],
            )
        })),
        _ => Err(bad_path()),
    }
}

fn cache(snapshot: &CapturedSnapshot, path: &[usize], offset: usize) -> Result<Page, Status> {
    let tiers = &snapshot.cache.tiers;
    match *path {
        [] => Ok(page(tiers, offset, |tier| {
            typed(
                format!("Tier {}", tier.tier_id),
                Model::CacheTier(api::CacheTier {
                    id: tier.tier_id,
                    fallback: tier.fallback,
                    events: tier.events,
                    hits: tier.hits,
                    misses: tier.misses,
                    errors: tier.errors,
                }),
                !tier.operations.is_empty(),
                &[],
            )
        })),
        [tier] => Ok(page(&at(tiers, tier)?.operations, offset, |operation| {
            row(format!("{:?}", operation.kind), fields!["Events" => operation.events])
        })),
        _ => Err(bad_path()),
    }
}
