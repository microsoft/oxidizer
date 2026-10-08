// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::time::{Instant, SystemTime};

pub(super) use super::runtime::{RuntimeMonitorSnapshot, RuntimeTaskSort, RuntimeTaskSummary, RuntimeWorkerSummary, runtime_task_id};

pub(super) struct CapturedSnapshot {
    pub(super) native: Option<Arc<seismograph_rallocator::native::Snapshot>>,
    pub(super) memory: Option<MemorySnapshot>,
    pub(super) allocations: Option<AllocationSnapshot>,
    pub(super) heap_error: Option<String>,
    pub(super) primitives: PrimitiveSnapshot,
    pub(super) runtime: RuntimeMonitorSnapshot,
    pub(super) io: IoMonitorSnapshot,
    pub(super) cache: CacheMonitorSnapshot,
    pub(super) threads: ThreadSnapshot,
    pub(super) task_events: super::task_events::TaskEventsSnapshot,
    pub(super) captured_at: Option<SystemTime>,
    pub(super) captured_instant: Option<Instant>,
    pub(super) filter_index: Option<Arc<super::filter_index::FilterIndex>>,
    pub(super) filter_summary: super::filter_index::FilterSummary,
}

pub(super) struct RuntimeSnapshot {
    pub(super) primitives: PrimitiveSnapshot,
    pub(super) runtime: RuntimeMonitorSnapshot,
    pub(super) io: IoMonitorSnapshot,
    pub(super) cache: CacheMonitorSnapshot,
    pub(super) threads: ThreadSnapshot,
    pub(super) task_events: super::task_events::TaskEventsSnapshot,
}

impl RuntimeSnapshot {
    #[cfg(test)]
    pub(super) fn from_events(
        decoded: &seismograph::snapshot::DecodedSnapshot,
        addresses: &[seismograph_rallocator::callers::AddressLookup],
        runtime_source: Option<&seismograph_runtime::snapshot::Snapshot>,
    ) -> Self {
        Self::from_events_with_progress(decoded, addresses, runtime_source, &mut |_| {})
    }

    pub(super) fn from_events_with_progress(
        decoded: &seismograph::snapshot::DecodedSnapshot,
        addresses: &[seismograph_rallocator::callers::AddressLookup],
        runtime_source: Option<&seismograph_runtime::snapshot::Snapshot>,
        progress: &mut impl FnMut(super::snapshot::Phase),
    ) -> Self {
        let task_events = super::task_events::TaskEventsSnapshot::from_events(&decoded.events, addresses, runtime_source);
        Self::from_events_with_attribution(decoded, addresses, runtime_source, task_events, progress)
    }

    pub(super) fn from_events_with_attribution(
        decoded: &seismograph::snapshot::DecodedSnapshot,
        addresses: &[seismograph_rallocator::callers::AddressLookup],
        runtime_source: Option<&seismograph_runtime::snapshot::Snapshot>,
        task_events: super::task_events::TaskEventsSnapshot,
        progress: &mut impl FnMut(super::snapshot::Phase),
    ) -> Self {
        use super::snapshot::Phase;
        progress(Phase::Primitives);
        let primitives = PrimitiveSnapshot::from_events(
            decoded.events.total_events,
            decoded.events.lost_events,
            &decoded.events.events,
            addresses,
        );
        progress(Phase::Runtime);
        let runtime = RuntimeMonitorSnapshot::from_events(&decoded.events, runtime_source, addresses);
        progress(Phase::Io);
        let io = IoMonitorSnapshot::from_events(&decoded.events);
        progress(Phase::Cache);
        let cache = CacheMonitorSnapshot::from_events(&decoded.events);
        progress(Phase::Threads);
        let threads = ThreadSnapshot::from_events(&decoded.events, addresses);
        Self {
            primitives,
            runtime,
            io,
            cache,
            threads,
            task_events,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct IoMonitorSnapshot {
    pub(super) total_events: u64,
    pub(super) retained_events: u64,
    pub(super) lost_events: u64,
    pub(super) resources: Vec<IoResourceSummary>,
}

impl IoMonitorSnapshot {
    #[expect(
        clippy::too_many_lines,
        reason = "I/O start and finish events are paired in one ordered pass so partial retained operations remain explicit"
    )]
    fn from_events(events: &seismograph::recorder::event::Events) -> Self {
        use seismograph::recorder::event::EventKind;
        use seismograph::recorder::io::IoOutcome;

        #[derive(Default)]
        struct ResourceBuilder {
            kind: Option<seismograph::recorder::io::IoResourceKind>,
            events: u64,
            reads: u64,
            writes: u64,
            completed: u64,
            errors: u64,
            canceled: u64,
            requested_bytes: u64,
            completed_bytes: u64,
        }

        #[derive(Default)]
        struct OperationBuilder {
            resource_id: u64,
            resource_kind: Option<seismograph::recorder::io::IoResourceKind>,
            kind: Option<IoOperationKind>,
            thread_id: u64,
            buffer_id: Option<u64>,
            requested_bytes: u64,
            completed_bytes: u64,
            buffer_len: u64,
            buffer_span_count: u32,
            outcome: Option<IoOutcome>,
            started_at: Option<u64>,
            finished_at: Option<u64>,
        }

        let mut resources = BTreeMap::<u64, ResourceBuilder>::new();
        let mut operations = BTreeMap::<u64, OperationBuilder>::new();
        let mut retained_events = 0_u64;
        for event in &events.events {
            let Some(io) = event.io() else {
                continue;
            };
            retained_events = retained_events.saturating_add(1);
            let resource_id = io.resource_id.get();
            let resource = resources.entry(resource_id).or_default();
            resource.kind = Some(io.resource_kind);
            resource.events = resource.events.saturating_add(1);

            let operation = operations.entry(io.operation_id.get()).or_default();
            operation.resource_id = resource_id;
            operation.resource_kind = Some(io.resource_kind);
            operation.thread_id = event.thread_id.get();
            operation.buffer_id = io.buffer_id.map(seismograph::recorder::io::BufferId::get);
            operation.requested_bytes = io.requested_bytes;
            operation.completed_bytes = io.completed_bytes;
            operation.buffer_len = io.buffer_len;
            operation.buffer_span_count = io.buffer_span_count;
            operation.outcome = Some(io.outcome);

            match event.kind {
                EventKind::IoReadStarted => {
                    resource.reads = resource.reads.saturating_add(1);
                    resource.requested_bytes = resource.requested_bytes.saturating_add(io.requested_bytes);
                    operation.kind = Some(IoOperationKind::Read);
                    operation.started_at = Some(event.timestamp.ticks());
                }
                EventKind::IoWriteStarted => {
                    resource.writes = resource.writes.saturating_add(1);
                    resource.requested_bytes = resource.requested_bytes.saturating_add(io.requested_bytes);
                    operation.kind = Some(IoOperationKind::Write);
                    operation.started_at = Some(event.timestamp.ticks());
                }
                EventKind::IoReadFinished | EventKind::IoWriteFinished => {
                    operation.kind = Some(if event.kind == EventKind::IoReadFinished {
                        IoOperationKind::Read
                    } else {
                        IoOperationKind::Write
                    });
                    operation.finished_at = Some(event.timestamp.ticks());
                    resource.completed = resource.completed.saturating_add(1);
                    resource.completed_bytes = resource.completed_bytes.saturating_add(io.completed_bytes);
                    resource.errors = resource.errors.saturating_add(u64::from(io.outcome == IoOutcome::Error));
                    resource.canceled = resource.canceled.saturating_add(u64::from(io.outcome == IoOutcome::Canceled));
                }
                _ => {}
            }
        }

        let mut operations_by_resource = BTreeMap::<u64, Vec<IoOperationSummary>>::new();
        for (operation_id, operation) in operations {
            let Some(kind) = operation.kind else {
                continue;
            };
            let resource_kind = operation
                .resource_kind
                .expect("resource kind is stored whenever an I/O operation builder is created");
            let outcome = operation
                .outcome
                .expect("outcome is stored whenever an I/O operation builder is created");
            operations_by_resource
                .entry(operation.resource_id)
                .or_default()
                .push(IoOperationSummary {
                    operation_id,
                    kind,
                    thread_id: operation.thread_id,
                    buffer_id: operation.buffer_id,
                    requested_bytes: operation.requested_bytes,
                    completed_bytes: operation.completed_bytes,
                    buffer_len: operation.buffer_len,
                    buffer_span_count: operation.buffer_span_count,
                    resource_kind,
                    outcome,
                    duration_nanos: operation
                        .started_at
                        .zip(operation.finished_at)
                        .map(|(started, finished)| finished.saturating_sub(started)),
                    timestamp: operation.finished_at.or(operation.started_at).unwrap_or_default(),
                });
        }
        for operations in operations_by_resource.values_mut() {
            operations.sort_unstable_by_key(|operation| std::cmp::Reverse((operation.timestamp, operation.operation_id)));
        }

        let mut resources = resources
            .into_iter()
            .filter_map(|(resource_id, resource)| {
                Some(IoResourceSummary {
                    resource_id,
                    kind: resource.kind?,
                    events: resource.events,
                    reads: resource.reads,
                    writes: resource.writes,
                    completed: resource.completed,
                    errors: resource.errors,
                    canceled: resource.canceled,
                    requested_bytes: resource.requested_bytes,
                    completed_bytes: resource.completed_bytes,
                    operations: operations_by_resource.remove(&resource_id).unwrap_or_default(),
                })
            })
            .collect::<Vec<_>>();
        resources.sort_unstable_by_key(|resource| std::cmp::Reverse((resource.completed_bytes, resource.events, resource.resource_id)));

        Self {
            total_events: events.total_events,
            retained_events,
            lost_events: events.lost_events,
            resources,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct IoResourceSummary {
    pub(super) resource_id: u64,
    pub(super) kind: seismograph::recorder::io::IoResourceKind,
    pub(super) events: u64,
    pub(super) reads: u64,
    pub(super) writes: u64,
    pub(super) completed: u64,
    pub(super) errors: u64,
    pub(super) canceled: u64,
    pub(super) requested_bytes: u64,
    pub(super) completed_bytes: u64,
    pub(super) operations: Vec<IoOperationSummary>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct IoOperationSummary {
    pub(super) operation_id: u64,
    pub(super) kind: IoOperationKind,
    pub(super) thread_id: u64,
    pub(super) buffer_id: Option<u64>,
    pub(super) requested_bytes: u64,
    pub(super) completed_bytes: u64,
    pub(super) buffer_len: u64,
    pub(super) buffer_span_count: u32,
    pub(super) resource_kind: seismograph::recorder::io::IoResourceKind,
    pub(super) outcome: seismograph::recorder::io::IoOutcome,
    pub(super) duration_nanos: Option<u64>,
    timestamp: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum IoOperationKind {
    Read,
    Write,
}

impl IoOperationKind {
    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Read => "Read",
            Self::Write => "Write",
        }
    }
}

const CACHE_EVENT_KINDS: [seismograph::recorder::event::EventKind; 22] = {
    use seismograph::recorder::event::EventKind;
    [
        EventKind::CacheHit,
        EventKind::CacheMiss,
        EventKind::CacheExpired,
        EventKind::CacheGetError,
        EventKind::CacheInserted,
        EventKind::CacheInsertRejected,
        EventKind::CacheInsertError,
        EventKind::CacheInvalidated,
        EventKind::CacheInvalidateError,
        EventKind::CacheCleared,
        EventKind::CacheClearError,
        EventKind::CacheRefreshHit,
        EventKind::CacheRefreshMiss,
        EventKind::CacheRefreshError,
        EventKind::CacheEvicted,
        EventKind::CacheComputeSucceeded,
        EventKind::CacheComputeFailed,
        EventKind::CacheComputeReturnedNone,
        EventKind::CachePromotionAccepted,
        EventKind::CachePromotionRejected,
        EventKind::CachePromotionFailed,
        EventKind::CacheRefreshSuppressed,
    ]
};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct CacheMonitorSnapshot {
    pub(super) total_events: u64,
    pub(super) retained_events: u64,
    pub(super) lost_events: u64,
    pub(super) tiers: Vec<CacheTierSummary>,
}

impl CacheMonitorSnapshot {
    fn from_events(events: &seismograph::recorder::event::Events) -> Self {
        use seismograph::recorder::event::EventKind;

        #[derive(Default)]
        struct TierBuilder {
            counts: [u64; CACHE_EVENT_KINDS.len()],
        }

        let mut tiers = BTreeMap::<(u64, bool), TierBuilder>::new();
        let mut retained_events = 0_u64;
        for event in &events.events {
            let Some(index) = CACHE_EVENT_KINDS.iter().position(|kind| *kind == event.kind) else {
                continue;
            };
            let Some(tier_id) = event.object_id().map(seismograph::recorder::event::ObjectId::get) else {
                continue;
            };
            let fallback = event.measurement().is_some_and(|value| value != 0);
            let tier = tiers.entry((tier_id, fallback)).or_default();
            tier.counts[index] = tier.counts[index].saturating_add(1);
            retained_events = retained_events.saturating_add(1);
        }

        let mut tiers = tiers
            .into_iter()
            .map(|((tier_id, fallback), tier)| {
                let count = |kind| {
                    CACHE_EVENT_KINDS
                        .iter()
                        .position(|candidate| *candidate == kind)
                        .map_or(0, |index| tier.counts[index])
                };
                let operations = CACHE_EVENT_KINDS
                    .into_iter()
                    .zip(tier.counts)
                    .filter_map(|(kind, events)| (events != 0).then_some(CacheOperationSummary { kind, events }))
                    .collect::<Vec<_>>();
                CacheTierSummary {
                    tier_id,
                    fallback,
                    events: operations.iter().map(|operation| operation.events).sum(),
                    hits: count(EventKind::CacheHit).saturating_add(count(EventKind::CacheRefreshHit)),
                    misses: count(EventKind::CacheMiss)
                        .saturating_add(count(EventKind::CacheExpired))
                        .saturating_add(count(EventKind::CacheRefreshMiss))
                        .saturating_add(count(EventKind::CacheComputeReturnedNone)),
                    errors: count(EventKind::CacheGetError)
                        .saturating_add(count(EventKind::CacheInsertError))
                        .saturating_add(count(EventKind::CacheInvalidateError))
                        .saturating_add(count(EventKind::CacheClearError))
                        .saturating_add(count(EventKind::CacheRefreshError))
                        .saturating_add(count(EventKind::CacheComputeFailed))
                        .saturating_add(count(EventKind::CachePromotionFailed)),
                    operations,
                }
            })
            .collect::<Vec<_>>();
        tiers.sort_unstable_by_key(|tier| std::cmp::Reverse((tier.events, tier.tier_id, tier.fallback)));

        Self {
            total_events: events.total_events,
            retained_events,
            lost_events: events.lost_events,
            tiers,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct CacheTierSummary {
    pub(super) tier_id: u64,
    pub(super) fallback: bool,
    pub(super) events: u64,
    pub(super) hits: u64,
    pub(super) misses: u64,
    pub(super) errors: u64,
    pub(super) operations: Vec<CacheOperationSummary>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct CacheOperationSummary {
    pub(super) kind: seismograph::recorder::event::EventKind,
    pub(super) events: u64,
}

pub(super) const fn cache_event_label(kind: seismograph::recorder::event::EventKind) -> &'static str {
    use seismograph::recorder::event::EventKind;
    match kind {
        EventKind::CacheHit => "Hit",
        EventKind::CacheMiss => "Miss",
        EventKind::CacheExpired => "Expired",
        EventKind::CacheGetError => "Get error",
        EventKind::CacheInserted => "Inserted",
        EventKind::CacheInsertRejected => "Insert rejected",
        EventKind::CacheInsertError => "Insert error",
        EventKind::CacheInvalidated => "Invalidated",
        EventKind::CacheInvalidateError => "Invalidate error",
        EventKind::CacheCleared => "Cleared",
        EventKind::CacheClearError => "Clear error",
        EventKind::CacheRefreshHit => "Refresh hit",
        EventKind::CacheRefreshMiss => "Refresh miss",
        EventKind::CacheRefreshError => "Refresh error",
        EventKind::CacheEvicted => "Evicted",
        EventKind::CacheComputeSucceeded => "Compute succeeded",
        EventKind::CacheComputeFailed => "Compute failed",
        EventKind::CacheComputeReturnedNone => "Compute returned none",
        EventKind::CachePromotionAccepted => "Promotion accepted",
        EventKind::CachePromotionRejected => "Promotion rejected",
        EventKind::CachePromotionFailed => "Promotion failed",
        EventKind::CacheRefreshSuppressed => "Refresh suppressed",
        _ => "Unknown",
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct AllocationSnapshot {
    pub(super) records: Vec<AllocationRecord>,
    pub(super) thread_count: u64,
    pub(super) total_events: u64,
    pub(super) retained_events: u64,
    pub(super) lost_events: u64,
    pub(super) hotspots: Vec<AllocationHotspot>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct AllocationRecord {
    operation: &'static str,
    correlation: &'static str,
    actor_id: u64,
    actor_name: Arc<str>,
    sequence: u64,
    lifetime_id: u64,
    origin_log: u64,
    address: u64,
    size: u64,
    alignment: u64,
    heap_key: u64,
    application_stack: Arc<[String]>,
    complete_stack: Arc<[String]>,
}

impl AllocationRecord {
    pub(super) fn label(&self) -> String {
        format!(
            "{} · {} #{} · {} B @ 0x{:x} · view #{} · {}",
            self.operation, self.actor_name, self.actor_id, self.size, self.address, self.lifetime_id, self.correlation
        )
    }

    pub(super) fn details(&self) -> Vec<String> {
        vec![
            format!(
                "Operation: {} · actor recorder thread {} ({}) · sequence {}",
                self.operation, self.actor_id, self.actor_name, self.sequence
            ),
            format!(
                "View-local lifetime #{} · origin recorder log {} · {}",
                self.lifetime_id, self.origin_log, self.correlation
            ),
            format!(
                "Address 0x{:x} · requested {} B · alignment {} B · recorded heap key {}",
                self.address, self.size, self.alignment, self.heap_key
            ),
            "Addresses and source correlation keys may repeat. Pairing uses timestamp, recorder thread, then sequence order, not global lifetime identity."
                .into(),
        ]
    }

    pub(super) fn stack(&self, filter: AllocationStackFilter) -> &[String] {
        match filter {
            AllocationStackFilter::Application => &self.application_stack,
            AllocationStackFilter::All => &self.complete_stack,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct AllocationHotspot {
    pub(super) allocations: u64,
    pub(super) allocated_bytes: u64,
    pub(super) live_allocations: u64,
    pub(super) live_bytes: u64,
    application_stack: Vec<String>,
    complete_stack: Vec<String>,
}

impl AllocationHotspot {
    pub(super) fn stack(&self, filter: AllocationStackFilter) -> &[String] {
        match filter {
            AllocationStackFilter::Application => &self.application_stack,
            AllocationStackFilter::All => &self.complete_stack,
        }
    }

    pub(super) fn location(&self, filter: AllocationStackFilter) -> &str {
        self.stack(filter).first().map_or("Backtraces disabled", String::as_str)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum AllocationStackFilter {
    Application,
    All,
}

impl AllocationStackFilter {
    pub(super) const fn toggle(self) -> Self {
        match self {
            Self::Application => Self::All,
            Self::All => Self::Application,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct PrimitiveSnapshot {
    pub(super) total_events: u64,
    pub(super) lost_events: u64,
    pub(super) groups: Vec<PrimitiveGroup>,
}

impl PrimitiveSnapshot {
    fn from_events(
        total_events: u64,
        lost_events: u64,
        events: &[seismograph::recorder::event::Event],
        addresses: &[seismograph_rallocator::callers::AddressLookup],
    ) -> Self {
        let lookups = addresses.iter().map(|lookup| (lookup.address, lookup)).collect::<HashMap<_, _>>();
        let mut selected = [false; 256];
        for kind in PrimitiveKind::ALL {
            for operation in kind.operations() {
                selected[usize::from(operation.event_kind().wire_value())] = true;
            }
        }
        let mut events_by_kind: [Vec<&seismograph::recorder::event::Event>; 256] = std::array::from_fn(|_| Vec::new());
        for event in events {
            let kind = usize::from(event.kind.wire_value());
            if selected[kind] {
                events_by_kind[kind].push(event);
            }
        }
        let groups = PrimitiveKind::ALL
            .into_iter()
            .map(|kind| PrimitiveGroup::from_events(kind, &events_by_kind, &lookups))
            .collect();
        Self {
            total_events,
            lost_events,
            groups,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct PrimitiveGroup {
    pub(super) kind: PrimitiveKind,
    pub(super) events: u64,
    pub(super) objects: u64,
    pub(super) contentions: u64,
    pub(super) operations: Vec<PrimitiveOperation>,
}

impl PrimitiveGroup {
    fn from_events(
        kind: PrimitiveKind,
        events_by_kind: &[Vec<&seismograph::recorder::event::Event>; 256],
        lookups: &HashMap<u64, &seismograph_rallocator::callers::AddressLookup>,
    ) -> Self {
        let object_ids = kind
            .operations()
            .iter()
            .filter(|operation| !operation.is_lock_poison())
            .flat_map(|operation| &events_by_kind[usize::from(operation.event_kind().wire_value())])
            .filter_map(|event| event.object_id().map(seismograph::recorder::event::ObjectId::get))
            .collect::<HashSet<_>>();
        let operations = kind
            .operations()
            .iter()
            .copied()
            .map(|operation| {
                PrimitiveOperation::from_events(
                    operation,
                    &events_by_kind[usize::from(operation.event_kind().wire_value())],
                    lookups,
                    &object_ids,
                )
            })
            .collect::<Vec<_>>();
        Self {
            kind,
            events: operations.iter().map(|operation| operation.events).sum(),
            objects: u64::try_from(object_ids.len()).unwrap_or(u64::MAX),
            contentions: operations
                .iter()
                .filter(|operation| operation.kind.is_contention())
                .map(|operation| operation.events)
                .sum(),
            operations,
        }
    }

    pub(super) fn sorted_operations(&self, sort: PrimitiveSort, descending: bool) -> Vec<&PrimitiveOperation> {
        let mut operations = self.operations.iter().collect::<Vec<_>>();
        operations.sort_unstable_by(|left, right| {
            let ordering = match sort {
                PrimitiveSort::Events => left.events.cmp(&right.events),
                PrimitiveSort::Objects => left.objects.cmp(&right.objects),
                PrimitiveSort::Threads => left.threads.cmp(&right.threads),
                PrimitiveSort::Hotspots => left.hotspots.len().cmp(&right.hotspots.len()),
            }
            .then_with(|| left.events.cmp(&right.events));
            if descending { ordering.reverse() } else { ordering }
        });
        operations
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct PrimitiveOperation {
    pub(super) kind: PrimitiveOperationKind,
    pub(super) events: u64,
    pub(super) objects: u64,
    pub(super) threads: u64,
    pub(super) hotspots: Vec<PrimitiveHotspot>,
}

impl PrimitiveOperation {
    fn from_events(
        kind: PrimitiveOperationKind,
        events: &[&seismograph::recorder::event::Event],
        lookups: &HashMap<u64, &seismograph_rallocator::callers::AddressLookup>,
        object_ids: &HashSet<u64>,
    ) -> Self {
        let matching = events
            .iter()
            .copied()
            .filter(|event| !kind.is_lock_poison() || event.object_id().is_some_and(|object_id| object_ids.contains(&object_id.get())))
            .collect::<Vec<_>>();
        let objects = matching
            .iter()
            .filter_map(|event| event.object_id().map(seismograph::recorder::event::ObjectId::get))
            .collect::<std::collections::HashSet<_>>();
        let threads = matching
            .iter()
            .map(|event| event.thread_id.get())
            .collect::<std::collections::HashSet<_>>();
        let mut totals = HashMap::<&[seismograph::recorder::event::Address], u64>::new();
        for event in &matching {
            *totals.entry(&event.call_stack).or_default() += 1;
        }
        let mut hotspots = totals
            .into_iter()
            .map(|(stack, count)| {
                let stack = stack.iter().map(|address| address.get()).collect::<Vec<_>>();
                PrimitiveHotspot {
                    count,
                    application_stack: primitive_stack(&stack, lookups, AllocationStackFilter::Application),
                    complete_stack: primitive_stack(&stack, lookups, AllocationStackFilter::All),
                }
            })
            .collect::<Vec<_>>();
        hotspots.sort_unstable_by_key(|hotspot| std::cmp::Reverse(hotspot.count));
        Self {
            kind,
            events: u64::try_from(matching.len()).unwrap_or(u64::MAX),
            objects: u64::try_from(objects.len()).unwrap_or(u64::MAX),
            threads: u64::try_from(threads.len()).unwrap_or(u64::MAX),
            hotspots,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct PrimitiveHotspot {
    pub(super) count: u64,
    application_stack: Vec<String>,
    complete_stack: Vec<String>,
}

impl PrimitiveHotspot {
    pub(super) fn stack(&self, filter: AllocationStackFilter) -> &[String] {
        match filter {
            AllocationStackFilter::Application => &self.application_stack,
            AllocationStackFilter::All => &self.complete_stack,
        }
    }

    pub(super) fn location(&self, filter: AllocationStackFilter) -> &str {
        self.stack(filter).first().map_or("Backtraces disabled", String::as_str)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PrimitiveKind {
    Arc,
    Mutex,
    RwLock,
    Barrier,
    Condvar,
    Once,
    Channel,
}

impl PrimitiveKind {
    const ALL: [Self; 7] = [
        Self::Arc,
        Self::Mutex,
        Self::RwLock,
        Self::Barrier,
        Self::Condvar,
        Self::Once,
        Self::Channel,
    ];

    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Arc => "Arc",
            Self::Mutex => "Mutex",
            Self::RwLock => "RwLock",
            Self::Barrier => "Barrier",
            Self::Condvar => "Condvar",
            Self::Once => "OnceLock / LazyLock",
            Self::Channel => "Channel",
        }
    }

    const fn operations(self) -> &'static [PrimitiveOperationKind] {
        match self {
            Self::Arc => &PrimitiveOperationKind::ARC,
            Self::Mutex => &PrimitiveOperationKind::MUTEX,
            Self::RwLock => &PrimitiveOperationKind::RW_LOCK,
            Self::Barrier => &PrimitiveOperationKind::BARRIER,
            Self::Condvar => &PrimitiveOperationKind::CONDVAR,
            Self::Once => &PrimitiveOperationKind::ONCE,
            Self::Channel => &PrimitiveOperationKind::CHANNEL,
        }
    }

    #[cfg(test)]
    fn identifies(self, kind: seismograph::recorder::event::EventKind) -> bool {
        self.operations()
            .iter()
            .filter(|operation| !operation.is_lock_poison())
            .any(|operation| operation.event_kind().wire_value() == kind.wire_value())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PrimitiveOperationKind {
    ArcCreate,
    ArcClone,
    ArcDeref,
    ArcDrop,
    ArcRelocate,
    MutexAccess,
    MutexContention,
    MutexRelease,
    RwLockReadAccess,
    RwLockReadContention,
    RwLockReadRelease,
    RwLockWriteAccess,
    RwLockWriteContention,
    RwLockWriteRelease,
    BarrierAccess,
    BarrierContention,
    BarrierRelease,
    CondvarAccess,
    CondvarContention,
    CondvarNotify,
    OnceAccess,
    OnceContention,
    OnceInitialize,
    ChannelSend,
    ChannelSendContention,
    ChannelReceive,
    ChannelReceiveContention,
    ChannelClose,
    ChannelHighWatermark,
    LockPoisoned,
    LockPoisonObserved,
    LockPoisonCleared,
}

impl PrimitiveOperationKind {
    const ARC: [Self; 5] = [Self::ArcCreate, Self::ArcClone, Self::ArcDeref, Self::ArcDrop, Self::ArcRelocate];
    const MUTEX: [Self; 6] = [
        Self::MutexAccess,
        Self::MutexContention,
        Self::MutexRelease,
        Self::LockPoisoned,
        Self::LockPoisonObserved,
        Self::LockPoisonCleared,
    ];
    const RW_LOCK: [Self; 9] = [
        Self::RwLockReadAccess,
        Self::RwLockReadContention,
        Self::RwLockReadRelease,
        Self::RwLockWriteAccess,
        Self::RwLockWriteContention,
        Self::RwLockWriteRelease,
        Self::LockPoisoned,
        Self::LockPoisonObserved,
        Self::LockPoisonCleared,
    ];
    const BARRIER: [Self; 3] = [Self::BarrierAccess, Self::BarrierContention, Self::BarrierRelease];
    const CONDVAR: [Self; 3] = [Self::CondvarAccess, Self::CondvarContention, Self::CondvarNotify];
    const ONCE: [Self; 3] = [Self::OnceAccess, Self::OnceContention, Self::OnceInitialize];
    const CHANNEL: [Self; 6] = [
        Self::ChannelSend,
        Self::ChannelSendContention,
        Self::ChannelReceive,
        Self::ChannelReceiveContention,
        Self::ChannelClose,
        Self::ChannelHighWatermark,
    ];

    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::ArcCreate => "Create",
            Self::ArcClone => "Clone",
            Self::ArcDeref => "Deref",
            Self::ArcDrop => "Final drop",
            Self::ArcRelocate => "Relocate",
            Self::MutexAccess => "Acquisition",
            Self::MutexContention | Self::OnceContention => "Contention",
            Self::MutexRelease => "Release",
            Self::RwLockReadAccess => "Read acquisition",
            Self::RwLockReadContention => "Read contention",
            Self::RwLockReadRelease => "Read release",
            Self::RwLockWriteAccess => "Write acquisition",
            Self::RwLockWriteContention => "Write contention",
            Self::RwLockWriteRelease => "Write release",
            Self::BarrierAccess | Self::CondvarAccess => "Completed wait",
            Self::BarrierContention | Self::CondvarContention => "Blocked wait",
            Self::BarrierRelease => "Generation release",
            Self::CondvarNotify => "Notification",
            Self::OnceAccess => "Access",
            Self::OnceInitialize => "Initialization",
            Self::ChannelSend => "Send",
            Self::ChannelReceive => "Receive",
            Self::ChannelSendContention => "Send contention",
            Self::ChannelReceiveContention => "Receive wait (empty)",
            Self::ChannelClose => "Close",
            Self::ChannelHighWatermark => "High watermark",
            Self::LockPoisoned => "Poisoned",
            Self::LockPoisonObserved => "Poison observed",
            Self::LockPoisonCleared => "Poison cleared",
        }
    }

    const fn event_kind(self) -> seismograph::recorder::event::EventKind {
        use seismograph::recorder::event::EventKind;
        match self {
            Self::ArcCreate => EventKind::ArcCreate,
            Self::ArcClone => EventKind::ArcClone,
            Self::ArcDeref => EventKind::ArcDeref,
            Self::ArcDrop => EventKind::ArcDrop,
            Self::ArcRelocate => EventKind::ArcRelocate,
            Self::MutexAccess => EventKind::MutexAccess,
            Self::MutexContention => EventKind::MutexContention,
            Self::MutexRelease => EventKind::MutexRelease,
            Self::RwLockReadAccess => EventKind::RwLockReadAccess,
            Self::RwLockReadContention => EventKind::RwLockReadContention,
            Self::RwLockReadRelease => EventKind::RwLockReadRelease,
            Self::RwLockWriteAccess => EventKind::RwLockWriteAccess,
            Self::RwLockWriteContention => EventKind::RwLockWriteContention,
            Self::RwLockWriteRelease => EventKind::RwLockWriteRelease,
            Self::BarrierAccess => EventKind::BarrierAccess,
            Self::BarrierContention => EventKind::BarrierContention,
            Self::BarrierRelease => EventKind::BarrierRelease,
            Self::CondvarAccess => EventKind::CondvarAccess,
            Self::CondvarContention => EventKind::CondvarContention,
            Self::CondvarNotify => EventKind::CondvarNotify,
            Self::OnceAccess => EventKind::OnceAccess,
            Self::OnceContention => EventKind::OnceContention,
            Self::OnceInitialize => EventKind::OnceInitialize,
            Self::ChannelSend => EventKind::ChannelSend,
            Self::ChannelSendContention => EventKind::ChannelSendContention,
            Self::ChannelReceive => EventKind::ChannelReceive,
            Self::ChannelReceiveContention => EventKind::ChannelReceiveContention,
            Self::ChannelClose => EventKind::ChannelClose,
            Self::ChannelHighWatermark => EventKind::ChannelHighWatermark,
            Self::LockPoisoned => EventKind::LockPoisoned,
            Self::LockPoisonObserved => EventKind::LockPoisonObserved,
            Self::LockPoisonCleared => EventKind::LockPoisonCleared,
        }
    }

    const fn is_lock_poison(self) -> bool {
        matches!(self, Self::LockPoisoned | Self::LockPoisonObserved | Self::LockPoisonCleared)
    }

    pub(super) const fn is_contention(self) -> bool {
        matches!(
            self,
            Self::MutexContention
                | Self::RwLockReadContention
                | Self::RwLockWriteContention
                | Self::BarrierContention
                | Self::CondvarContention
                | Self::OnceContention
                | Self::ChannelSendContention
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PrimitiveSort {
    Events,
    Objects,
    Threads,
    Hotspots,
}

impl PrimitiveSort {
    pub(super) const fn next(self) -> Self {
        match self {
            Self::Events => Self::Objects,
            Self::Objects => Self::Threads,
            Self::Threads => Self::Hotspots,
            Self::Hotspots => Self::Events,
        }
    }

    pub(super) const fn previous(self) -> Self {
        match self {
            Self::Events => Self::Hotspots,
            Self::Objects => Self::Events,
            Self::Threads => Self::Objects,
            Self::Hotspots => Self::Threads,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ThreadSnapshot {
    pub(super) threads: Vec<ThreadSummary>,
}

impl ThreadSnapshot {
    fn from_events(decoded: &seismograph::recorder::event::Events, addresses: &[seismograph_rallocator::callers::AddressLookup]) -> Self {
        let mut operations_by_kind = [None; 256];
        for (index, kind) in ThreadOperationKind::ALL.into_iter().enumerate() {
            operations_by_kind[usize::from(kind.event_kind().wire_value())] = Some(index);
        }
        let mut threads = decoded
            .threads
            .iter()
            .map(|thread| {
                let mut summary = empty_thread_summary(thread.thread_id.get());
                summary.name.clone_from(&thread.name);
                summary.total_events = thread.total_events;
                summary.lost_events = thread.lost_events;
                (summary.thread_id, summary)
            })
            .collect::<HashMap<_, _>>();
        let mut objects = Vec::new();
        for event in &decoded.events {
            let id = event.thread_id.get();
            let thread = threads.entry(id).or_insert_with(|| empty_thread_summary(id));
            thread.retained_events += 1;
            if let Some(operation) = operations_by_kind[usize::from(event.kind.wire_value())] {
                thread.operations[operation].events += 1;
                if let Some(object) = event.object_id() {
                    objects.push(ThreadObjectEvent {
                        object_id: object.get(),
                        event,
                    });
                }
            }
        }
        // A flat index replaces per-object vectors and per-thread object maps. Cache the
        // object ID beside the reference so sorting does not chase the full event payload.
        objects.sort_unstable_by(|left, right| {
            left.object_id
                .cmp(&right.object_id)
                .then_with(|| left.event.thread_id.get().cmp(&right.event.thread_id.get()))
                .then_with(|| left.event.kind.wire_value().cmp(&right.event.kind.wire_value()))
        });
        let mut stacks = ThreadStacks::new(addresses);
        let relations = accumulate_thread_objects(&objects, &mut threads, &operations_by_kind, &mut stacks);
        for ((thread_id, operation, _), mut participant) in relations {
            participant.objects.sort_unstable_by(|left, right| {
                right
                    .hotness()
                    .cmp(&left.hotness())
                    .then_with(|| right.related_events.cmp(&left.related_events))
                    .then_with(|| left.object_id.cmp(&right.object_id))
            });
            threads
                .get_mut(&thread_id)
                .unwrap_or_else(|| unreachable!("relations only refer to indexed threads"))
                .operations[operation]
                .participants
                .push(participant);
        }
        let mut threads = threads
            .into_values()
            .map(|mut thread| {
                thread.total_events = thread.total_events.max(thread.retained_events);
                for operation in &mut thread.operations {
                    operation.participants.sort_unstable_by_key(|participant| participant.thread_id);
                }
                thread
            })
            .collect::<Vec<_>>();
        threads.sort_unstable_by_key(|thread| thread.thread_id);
        Self { threads }
    }

    #[cfg(test)]
    fn from_events_reference(
        decoded: &seismograph::recorder::event::Events,
        addresses: &[seismograph_rallocator::callers::AddressLookup],
    ) -> Self {
        #[derive(Default)]
        struct Metadata {
            name: String,
            total_events: u64,
            lost_events: u64,
        }

        let mut stacks = ThreadStacks::new(addresses);
        let mut metadata = decoded
            .threads
            .iter()
            .map(|thread| {
                (
                    thread.thread_id.get(),
                    Metadata {
                        name: thread.name.clone(),
                        total_events: thread.total_events,
                        lost_events: thread.lost_events,
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        let mut events_by_thread = HashMap::<u64, Vec<&seismograph::recorder::event::Event>>::new();
        let mut events_by_object = HashMap::<u64, Vec<&seismograph::recorder::event::Event>>::new();
        for event in &decoded.events {
            metadata.entry(event.thread_id.get()).or_default();
            events_by_thread.entry(event.thread_id.get()).or_default().push(event);
            if let Some(object_id) = event.object_id() {
                events_by_object.entry(object_id.get()).or_default().push(event);
            }
        }
        let threads = metadata
            .into_iter()
            .map(|(thread_id, metadata)| {
                let events = events_by_thread.get(&thread_id).map_or(&[][..], Vec::as_slice);
                let retained_events = u64::try_from(events.len()).unwrap_or(u64::MAX);
                let operations = ThreadOperationKind::ALL
                    .into_iter()
                    .map(|kind| ThreadOperation::from_events(kind, events, &events_by_object, &decoded.threads, &mut stacks))
                    .collect();
                ThreadSummary {
                    thread_id,
                    name: metadata.name,
                    total_events: metadata.total_events.max(retained_events),
                    retained_events,
                    lost_events: metadata.lost_events,
                    operations,
                }
            })
            .collect();
        Self { threads }
    }
}

fn empty_thread_summary(thread_id: u64) -> ThreadSummary {
    ThreadSummary {
        thread_id,
        name: String::new(),
        total_events: 0,
        retained_events: 0,
        lost_events: 0,
        operations: ThreadOperationKind::ALL
            .into_iter()
            .map(|kind| ThreadOperation {
                kind,
                events: 0,
                objects: 0,
                participants: Vec::new(),
            })
            .collect(),
    }
}

struct ThreadObjectEvent<'a> {
    object_id: u64,
    event: &'a seismograph::recorder::event::Event,
}

fn accumulate_thread_objects<'a>(
    objects: &[ThreadObjectEvent<'a>],
    threads: &mut HashMap<u64, ThreadSummary>,
    operations_by_kind: &[Option<usize>; 256],
    stacks: &mut ThreadStacks<'a>,
) -> HashMap<(u64, usize, u64), ThreadParticipant> {
    let mut relations = HashMap::<(u64, usize, u64), ThreadParticipant>::new();
    for object in objects.chunk_by(|left, right| left.object_id == right.object_id) {
        for selected in object.chunk_by(|left, right| left.event.thread_id == right.event.thread_id && left.event.kind == right.event.kind)
        {
            let first = &selected[0];
            let thread_id = first.event.thread_id.get();
            let operation =
                operations_by_kind[usize::from(first.event.kind.wire_value())].expect("the index contains only supported operation kinds");
            let kind = ThreadOperationKind::ALL[operation];
            threads
                .get_mut(&thread_id)
                .unwrap_or_else(|| unreachable!("all event threads were indexed"))
                .operations[operation]
                .objects += 1;
            let mut selected_stacks = None;
            for related in object.chunk_by(|left, right| left.event.thread_id == right.event.thread_id) {
                let participant_id = related[0].event.thread_id.get();
                let related_events = related
                    .iter()
                    .filter(|event| kind.is_related(event.event.kind))
                    .map(|event| event.event);
                let count = u64::try_from(related_events.clone().count()).unwrap_or(u64::MAX);
                if count == 0 {
                    continue;
                }
                let selected_stacks =
                    selected_stacks.get_or_insert_with(|| thread_stacks(selected.iter().map(|event| event.event), kind, stacks));
                let participant = relations
                    .entry((thread_id, operation, participant_id))
                    .or_insert_with(|| ThreadParticipant {
                        thread_id: participant_id,
                        name: threads.get(&participant_id).map_or_else(String::new, |thread| thread.name.clone()),
                        events: 0,
                        objects: Vec::new(),
                    });
                participant.events = participant.events.saturating_add(count);
                participant.objects.push(ThreadObject {
                    object_id: first.object_id,
                    selected_events: u64::try_from(selected.len()).unwrap_or(u64::MAX),
                    related_events: count,
                    selected_stacks: selected_stacks.clone(),
                    related_stacks: thread_stacks(related_events, kind, stacks),
                });
            }
        }
    }
    relations
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ThreadSummary {
    pub(super) thread_id: u64,
    pub(super) name: String,
    pub(super) total_events: u64,
    pub(super) retained_events: u64,
    pub(super) lost_events: u64,
    pub(super) operations: Vec<ThreadOperation>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ThreadOperation {
    pub(super) kind: ThreadOperationKind,
    pub(super) events: u64,
    pub(super) objects: u64,
    pub(super) participants: Vec<ThreadParticipant>,
}

impl ThreadOperation {
    #[cfg(test)]
    fn from_events<'a>(
        kind: ThreadOperationKind,
        events: &[&'a seismograph::recorder::event::Event],
        events_by_object: &HashMap<u64, Vec<&'a seismograph::recorder::event::Event>>,
        thread_logs: &[seismograph::recorder::thread::ThreadLog],
        stacks: &mut ThreadStacks<'a>,
    ) -> Self {
        #[derive(Default)]
        struct ParticipantTotal<'a> {
            objects: BTreeMap<u64, Vec<&'a seismograph::recorder::event::Event>>,
            events: u64,
        }

        let matching = events
            .iter()
            .copied()
            .filter(|event| event.kind == kind.event_kind())
            .collect::<Vec<_>>();
        let mut selected_by_object = HashMap::<u64, Vec<&seismograph::recorder::event::Event>>::new();
        for event in &matching {
            if let Some(object_id) = event.object_id() {
                selected_by_object.entry(object_id.get()).or_default().push(event);
            }
        }
        let mut totals = BTreeMap::<u64, ParticipantTotal<'_>>::new();
        for object_id in selected_by_object.keys() {
            for event in events_by_object.get(object_id).into_iter().flatten() {
                let participant_id = event.thread_id.get();
                if !kind.is_related(event.kind) {
                    continue;
                }
                let total = totals.entry(participant_id).or_default();
                total.objects.entry(*object_id).or_default().push(*event);
                total.events = total.events.saturating_add(1);
            }
        }
        let participants = totals
            .into_iter()
            .map(|(participant_id, total)| {
                let mut participant_objects = total
                    .objects
                    .into_iter()
                    .map(|(object_id, related_events)| {
                        let selected_events = selected_by_object.get(&object_id).map_or(&[][..], Vec::as_slice);
                        ThreadObject::from_events(kind, object_id, selected_events, &related_events, stacks)
                    })
                    .collect::<Vec<_>>();
                participant_objects.sort_unstable_by(|left, right| {
                    right
                        .hotness()
                        .cmp(&left.hotness())
                        .then_with(|| right.related_events.cmp(&left.related_events))
                        .then_with(|| left.object_id.cmp(&right.object_id))
                });
                ThreadParticipant {
                    thread_id: participant_id,
                    name: thread_logs
                        .iter()
                        .find(|thread| thread.thread_id.get() == participant_id)
                        .map_or_else(String::new, |thread| thread.name.clone()),
                    events: total.events,
                    objects: participant_objects,
                }
            })
            .collect();
        Self {
            kind,
            events: u64::try_from(matching.len()).unwrap_or(u64::MAX),
            objects: u64::try_from(selected_by_object.len()).unwrap_or(u64::MAX),
            participants,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ThreadParticipant {
    pub(super) thread_id: u64,
    pub(super) name: String,
    pub(super) events: u64,
    pub(super) objects: Vec<ThreadObject>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ThreadObject {
    pub(super) object_id: u64,
    pub(super) selected_events: u64,
    pub(super) related_events: u64,
    selected_stacks: ThreadStackSet,
    related_stacks: ThreadStackSet,
}

impl ThreadObject {
    #[cfg(test)]
    fn from_events<'a>(
        kind: ThreadOperationKind,
        object_id: u64,
        selected_events: &[&'a seismograph::recorder::event::Event],
        related_events: &[&'a seismograph::recorder::event::Event],
        stacks: &mut ThreadStacks<'a>,
    ) -> Self {
        Self {
            object_id,
            selected_events: u64::try_from(selected_events.len()).unwrap_or(u64::MAX),
            related_events: u64::try_from(related_events.len()).unwrap_or(u64::MAX),
            selected_stacks: thread_stacks(selected_events.iter().copied(), kind, stacks),
            related_stacks: thread_stacks(related_events.iter().copied(), kind, stacks),
        }
    }

    pub(super) const fn hotness(&self) -> u64 {
        self.selected_events.saturating_add(self.related_events)
    }

    pub(super) fn selected_stack(&self) -> Option<&ThreadStack> {
        self.selected_stacks.first()
    }

    pub(super) fn related_stack(&self) -> Option<&ThreadStack> {
        self.related_stacks.first()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ThreadStack {
    pub(super) count: u64,
    frames: Arc<ThreadFrames>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ThreadFrames {
    application: Vec<String>,
    complete: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ThreadStackSet {
    Empty,
    One(ThreadStack),
    Many(Vec<ThreadStack>),
}

impl ThreadStackSet {
    fn first(&self) -> Option<&ThreadStack> {
        match self {
            Self::Empty => None,
            Self::One(stack) => Some(stack),
            Self::Many(stacks) => stacks.first(),
        }
    }
}

impl From<Vec<ThreadStack>> for ThreadStackSet {
    fn from(mut stacks: Vec<ThreadStack>) -> Self {
        match stacks.len() {
            0 => Self::Empty,
            1 => Self::One(stacks.remove(0)),
            _ => Self::Many(stacks),
        }
    }
}

impl ThreadStack {
    pub(super) fn stack(&self, filter: AllocationStackFilter) -> &[String] {
        match filter {
            AllocationStackFilter::Application => &self.frames.application,
            AllocationStackFilter::All => &self.frames.complete,
        }
    }
}

/// Resolves each distinct stack once, even when millions of objects share it.
struct ThreadStacks<'a> {
    lookups: HashMap<u64, &'a seismograph_rallocator::callers::AddressLookup>,
    allocation: HashMap<&'a [seismograph::recorder::event::Address], ThreadStack>,
    primitive: HashMap<&'a [seismograph::recorder::event::Address], ThreadStack>,
}

impl<'a> ThreadStacks<'a> {
    fn new(addresses: &'a [seismograph_rallocator::callers::AddressLookup]) -> Self {
        Self {
            lookups: addresses.iter().map(|lookup| (lookup.address, lookup)).collect(),
            allocation: HashMap::new(),
            primitive: HashMap::new(),
        }
    }

    fn get(&mut self, addresses: &'a [seismograph::recorder::event::Address], kind: ThreadOperationKind, count: u64) -> ThreadStack {
        let cache = if kind.is_allocation() {
            &mut self.allocation
        } else {
            &mut self.primitive
        };
        let stack = cache.entry(addresses).or_insert_with(|| {
            let addresses = addresses.iter().map(|address| address.get()).collect::<Vec<_>>();
            let format = |filter| {
                if kind.is_allocation() {
                    hotspot_stack(&addresses, &self.lookups, filter)
                } else {
                    primitive_stack(&addresses, &self.lookups, filter)
                }
            };
            ThreadStack {
                count: 0,
                frames: Arc::new(ThreadFrames {
                    application: format(AllocationStackFilter::Application),
                    complete: format(AllocationStackFilter::All),
                }),
            }
        });
        ThreadStack { count, ..stack.clone() }
    }
}

fn thread_stacks<'a>(
    events: impl Iterator<Item = &'a seismograph::recorder::event::Event>,
    kind: ThreadOperationKind,
    cache: &mut ThreadStacks<'a>,
) -> ThreadStackSet {
    let mut events = events;
    let Some(first) = events.next() else {
        return ThreadStackSet::Empty;
    };
    let mut count = 1;
    let second = loop {
        let Some(event) = events.next() else {
            return ThreadStackSet::One(cache.get(&first.call_stack, kind, count));
        };
        if event.call_stack != first.call_stack {
            break event;
        }
        count += 1;
    };
    let mut totals = HashMap::<&[seismograph::recorder::event::Address], u64>::new();
    totals.insert(&first.call_stack, count);
    totals.insert(&second.call_stack, 1);
    for event in events {
        *totals.entry(&event.call_stack).or_default() += 1;
    }
    let mut stacks = totals
        .into_iter()
        .map(|(addresses, count)| cache.get(addresses, kind, count))
        .collect::<Vec<_>>();
    stacks.sort_unstable_by_key(|stack| std::cmp::Reverse(stack.count));
    stacks.into()
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) enum ThreadOperationKind {
    Allocation,
    Deallocation,
    ArcCreate,
    ArcClone,
    ArcDeref,
    ArcDrop,
    ArcRelocate,
    MutexAccess,
    MutexContention,
    MutexRelease,
    RwLockReadAccess,
    RwLockReadContention,
    RwLockReadRelease,
    RwLockWriteAccess,
    RwLockWriteContention,
    RwLockWriteRelease,
    BarrierAccess,
    BarrierContention,
    BarrierRelease,
    CondvarAccess,
    CondvarContention,
    CondvarNotify,
    OnceAccess,
    OnceContention,
    OnceInitialize,
    ChannelSend,
    ChannelSendContention,
    ChannelReceive,
    ChannelReceiveContention,
    ChannelClose,
    ChannelHighWatermark,
    LockPoisoned,
    LockPoisonObserved,
    LockPoisonCleared,
}

impl ThreadOperationKind {
    pub(super) const ALL: [Self; 34] = [
        Self::Allocation,
        Self::Deallocation,
        Self::ArcCreate,
        Self::ArcClone,
        Self::ArcDeref,
        Self::ArcDrop,
        Self::ArcRelocate,
        Self::MutexAccess,
        Self::MutexContention,
        Self::MutexRelease,
        Self::RwLockReadAccess,
        Self::RwLockReadContention,
        Self::RwLockReadRelease,
        Self::RwLockWriteAccess,
        Self::RwLockWriteContention,
        Self::RwLockWriteRelease,
        Self::BarrierAccess,
        Self::BarrierContention,
        Self::BarrierRelease,
        Self::CondvarAccess,
        Self::CondvarContention,
        Self::CondvarNotify,
        Self::OnceAccess,
        Self::OnceContention,
        Self::OnceInitialize,
        Self::ChannelSend,
        Self::ChannelSendContention,
        Self::ChannelReceive,
        Self::ChannelReceiveContention,
        Self::ChannelClose,
        Self::ChannelHighWatermark,
        Self::LockPoisoned,
        Self::LockPoisonObserved,
        Self::LockPoisonCleared,
    ];

    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Allocation => "Allocation",
            Self::Deallocation => "Deallocation",
            Self::ArcCreate => "Arc create",
            Self::ArcClone => "Arc clone",
            Self::ArcDeref => "Arc deref",
            Self::ArcDrop => "Arc final drop",
            Self::ArcRelocate => "Arc relocation",
            Self::MutexAccess => "Mutex acquisition",
            Self::MutexContention => "Mutex contention",
            Self::MutexRelease => "Mutex release",
            Self::RwLockReadAccess => "RwLock read acquisition",
            Self::RwLockReadContention => "RwLock read contention",
            Self::RwLockReadRelease => "RwLock read release",
            Self::RwLockWriteAccess => "RwLock write acquisition",
            Self::RwLockWriteContention => "RwLock write contention",
            Self::RwLockWriteRelease => "RwLock write release",
            Self::BarrierAccess => "Barrier completed wait",
            Self::BarrierContention => "Barrier blocked wait",
            Self::BarrierRelease => "Barrier generation release",
            Self::CondvarAccess => "Condvar completed wait",
            Self::CondvarContention => "Condvar blocked wait",
            Self::CondvarNotify => "Condvar notification",
            Self::OnceAccess => "Once value access",
            Self::OnceContention => "Once initialization contention",
            Self::OnceInitialize => "Once initialization",
            Self::ChannelSend => "Channel send",
            Self::ChannelSendContention => "Channel send contention",
            Self::ChannelReceive => "Channel receive",
            Self::ChannelReceiveContention => "Channel receive wait (empty)",
            Self::ChannelClose => "Channel close",
            Self::ChannelHighWatermark => "Channel high watermark",
            Self::LockPoisoned => "Lock poisoned",
            Self::LockPoisonObserved => "Lock poison observed",
            Self::LockPoisonCleared => "Lock poison cleared",
        }
    }

    pub(super) const fn relationship_label(self) -> &'static str {
        match self {
            Self::Allocation => "Threads that deallocated these allocations",
            Self::Deallocation => "Threads that created these allocations",
            Self::ArcCreate | Self::ArcClone | Self::ArcDeref | Self::ArcDrop | Self::ArcRelocate => {
                "Threads (including self) observed on the same Arc objects"
            }
            Self::MutexAccess | Self::MutexContention | Self::MutexRelease => "Threads (including self) observed on the same Mutex objects",
            Self::RwLockReadAccess
            | Self::RwLockReadContention
            | Self::RwLockReadRelease
            | Self::RwLockWriteAccess
            | Self::RwLockWriteContention
            | Self::RwLockWriteRelease => "Threads (including self) observed on the same RwLock objects",
            Self::BarrierAccess | Self::BarrierContention | Self::BarrierRelease => {
                "Threads (including self) observed on the same Barrier objects"
            }
            Self::CondvarAccess | Self::CondvarContention | Self::CondvarNotify => {
                "Threads (including self) observed on the same Condvar objects"
            }
            Self::OnceAccess | Self::OnceContention | Self::OnceInitialize => {
                "Threads (including self) observed on the same once-initialized objects"
            }
            Self::ChannelSend
            | Self::ChannelSendContention
            | Self::ChannelReceive
            | Self::ChannelReceiveContention
            | Self::ChannelClose
            | Self::ChannelHighWatermark => "Threads (including self) observed on the same Channel objects",
            Self::LockPoisoned | Self::LockPoisonObserved | Self::LockPoisonCleared => {
                "Threads (including self) observed on the same Mutex or RwLock objects"
            }
        }
    }

    pub(super) const fn is_contention(self) -> bool {
        matches!(
            self,
            Self::MutexContention
                | Self::RwLockReadContention
                | Self::RwLockWriteContention
                | Self::BarrierContention
                | Self::CondvarContention
                | Self::OnceContention
                | Self::ChannelSendContention
        )
    }

    pub(super) const fn is_allocation(self) -> bool {
        matches!(self, Self::Allocation | Self::Deallocation)
    }

    pub(super) const fn event_kind(self) -> seismograph::recorder::event::EventKind {
        use seismograph::recorder::event::EventKind;
        match self {
            Self::Allocation => EventKind::Allocation,
            Self::Deallocation => EventKind::Deallocation,
            Self::ArcCreate => EventKind::ArcCreate,
            Self::ArcClone => EventKind::ArcClone,
            Self::ArcDeref => EventKind::ArcDeref,
            Self::ArcDrop => EventKind::ArcDrop,
            Self::ArcRelocate => EventKind::ArcRelocate,
            Self::MutexAccess => EventKind::MutexAccess,
            Self::MutexContention => EventKind::MutexContention,
            Self::MutexRelease => EventKind::MutexRelease,
            Self::RwLockReadAccess => EventKind::RwLockReadAccess,
            Self::RwLockReadContention => EventKind::RwLockReadContention,
            Self::RwLockReadRelease => EventKind::RwLockReadRelease,
            Self::RwLockWriteAccess => EventKind::RwLockWriteAccess,
            Self::RwLockWriteContention => EventKind::RwLockWriteContention,
            Self::RwLockWriteRelease => EventKind::RwLockWriteRelease,
            Self::BarrierAccess => EventKind::BarrierAccess,
            Self::BarrierContention => EventKind::BarrierContention,
            Self::BarrierRelease => EventKind::BarrierRelease,
            Self::CondvarAccess => EventKind::CondvarAccess,
            Self::CondvarContention => EventKind::CondvarContention,
            Self::CondvarNotify => EventKind::CondvarNotify,
            Self::OnceAccess => EventKind::OnceAccess,
            Self::OnceContention => EventKind::OnceContention,
            Self::OnceInitialize => EventKind::OnceInitialize,
            Self::ChannelSend => EventKind::ChannelSend,
            Self::ChannelSendContention => EventKind::ChannelSendContention,
            Self::ChannelReceive => EventKind::ChannelReceive,
            Self::ChannelReceiveContention => EventKind::ChannelReceiveContention,
            Self::ChannelClose => EventKind::ChannelClose,
            Self::ChannelHighWatermark => EventKind::ChannelHighWatermark,
            Self::LockPoisoned => EventKind::LockPoisoned,
            Self::LockPoisonObserved => EventKind::LockPoisonObserved,
            Self::LockPoisonCleared => EventKind::LockPoisonCleared,
        }
    }

    const fn is_related(self, kind: seismograph::recorder::event::EventKind) -> bool {
        use seismograph::recorder::event::EventKind;
        match self {
            Self::Allocation => matches!(kind, EventKind::Deallocation),
            Self::Deallocation => matches!(kind, EventKind::Allocation),
            Self::ArcCreate | Self::ArcClone | Self::ArcDeref | Self::ArcDrop | Self::ArcRelocate => matches!(
                kind,
                EventKind::ArcCreate | EventKind::ArcClone | EventKind::ArcDeref | EventKind::ArcDrop | EventKind::ArcRelocate
            ),
            Self::MutexAccess | Self::MutexContention | Self::MutexRelease => {
                matches!(
                    kind,
                    EventKind::MutexAccess
                        | EventKind::MutexContention
                        | EventKind::MutexRelease
                        | EventKind::LockPoisoned
                        | EventKind::LockPoisonObserved
                        | EventKind::LockPoisonCleared
                )
            }
            Self::RwLockReadAccess
            | Self::RwLockReadContention
            | Self::RwLockReadRelease
            | Self::RwLockWriteAccess
            | Self::RwLockWriteContention
            | Self::RwLockWriteRelease => matches!(
                kind,
                EventKind::RwLockReadAccess
                    | EventKind::RwLockReadContention
                    | EventKind::RwLockReadRelease
                    | EventKind::RwLockWriteAccess
                    | EventKind::RwLockWriteContention
                    | EventKind::RwLockWriteRelease
                    | EventKind::LockPoisoned
                    | EventKind::LockPoisonObserved
                    | EventKind::LockPoisonCleared
            ),
            Self::BarrierAccess | Self::BarrierContention | Self::BarrierRelease => matches!(
                kind,
                EventKind::BarrierAccess | EventKind::BarrierContention | EventKind::BarrierRelease
            ),
            Self::CondvarAccess | Self::CondvarContention | Self::CondvarNotify => matches!(
                kind,
                EventKind::CondvarAccess | EventKind::CondvarContention | EventKind::CondvarNotify
            ),
            Self::OnceAccess | Self::OnceContention | Self::OnceInitialize => {
                matches!(kind, EventKind::OnceAccess | EventKind::OnceContention | EventKind::OnceInitialize)
            }
            Self::ChannelSend
            | Self::ChannelSendContention
            | Self::ChannelReceive
            | Self::ChannelReceiveContention
            | Self::ChannelClose
            | Self::ChannelHighWatermark => matches!(
                kind,
                EventKind::ChannelSend
                    | EventKind::ChannelSendContention
                    | EventKind::ChannelReceive
                    | EventKind::ChannelReceiveContention
                    | EventKind::ChannelClose
                    | EventKind::ChannelHighWatermark
            ),
            Self::LockPoisoned | Self::LockPoisonObserved | Self::LockPoisonCleared => matches!(
                kind,
                EventKind::MutexAccess
                    | EventKind::MutexContention
                    | EventKind::MutexRelease
                    | EventKind::RwLockReadAccess
                    | EventKind::RwLockReadContention
                    | EventKind::RwLockReadRelease
                    | EventKind::RwLockWriteAccess
                    | EventKind::RwLockWriteContention
                    | EventKind::RwLockWriteRelease
                    | EventKind::LockPoisoned
                    | EventKind::LockPoisonObserved
                    | EventKind::LockPoisonCleared
            ),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum AllocationSort {
    Allocations,
    AllocatedBytes,
    AverageBytes,
    LiveAllocations,
    LiveBytes,
}

impl AllocationSort {
    pub(super) const fn next(self) -> Self {
        match self {
            Self::Allocations => Self::AllocatedBytes,
            Self::AllocatedBytes => Self::AverageBytes,
            Self::AverageBytes => Self::LiveAllocations,
            Self::LiveAllocations => Self::LiveBytes,
            Self::LiveBytes => Self::Allocations,
        }
    }

    pub(super) const fn previous(self) -> Self {
        match self {
            Self::Allocations => Self::LiveBytes,
            Self::AllocatedBytes => Self::Allocations,
            Self::AverageBytes => Self::AllocatedBytes,
            Self::LiveAllocations => Self::AverageBytes,
            Self::LiveBytes => Self::LiveAllocations,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct MemorySnapshot {
    pub(super) live_bytes: u64,
    pub(super) peak_live_bytes: u64,
    pub(super) peak_live_bytes_scope: crate::allocator_view::PeakLiveBytesScope,
    pub(super) mapped_bytes: u64,
    pub(super) allocations: u64,
    pub(super) reserved_bytes: u64,
    pub(super) used_slices: u64,
    pub(super) free_slices: u64,
    pub(super) slice_bytes: u64,
    pub(super) small_slices: u64,
    pub(super) medium_slices: u64,
    pub(super) bump_slices: u64,
    pub(super) unknown_slices: u64,
    pub(super) regions: Vec<MemoryRegion>,
    pub(super) size_classes: Vec<MemorySizeClass>,
    pub(super) medium_allocations: MediumAllocations,
    pub(super) tiers: Vec<MemoryTierData>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct MemoryRegion {
    pub(super) index: u32,
    pub(super) reserved_bytes: u64,
    pub(super) used_slices: u64,
    pub(super) free_slices: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct MemorySizeClass {
    pub(super) block_bytes: u64,
    pub(super) live_allocations: u64,
    pub(super) capacity_blocks: u64,
    pub(super) requested_bytes: u64,
    pub(super) usable_bytes: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct MediumAllocations {
    pub(super) count: u64,
    pub(super) requested_bytes: u64,
    pub(super) usable_bytes: u64,
    pub(super) span_slices: u64,
    pub(super) largest_requested_bytes: u64,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) enum MemoryTier {
    Small,
    Medium,
    Direct,
}

impl MemoryTier {
    const ALL: [Self; 3] = [Self::Small, Self::Medium, Self::Direct];

    pub(super) const fn next(self) -> Self {
        match self {
            Self::Small => Self::Medium,
            Self::Medium => Self::Direct,
            Self::Direct => Self::Small,
        }
    }

    pub(super) const fn previous(self) -> Self {
        match self {
            Self::Small => Self::Direct,
            Self::Medium => Self::Small,
            Self::Direct => Self::Medium,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct MemoryTierData {
    pub(super) kind: MemoryTier,
    pub(super) current_allocations: u64,
    pub(super) current_bytes: u64,
    pub(super) buckets: Vec<MemoryBucket>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct MemoryBucket {
    pub(super) lower_bytes: u64,
    pub(super) upper_bytes: u64,
    pub(super) allocations: u64,
    pub(super) allocated_bytes: u64,
    pub(super) live_allocations: u64,
    pub(super) live_bytes: u64,
    pub(super) topology_live_allocations: Option<u64>,
    pub(super) capacity_blocks: Option<u64>,
    pub(super) requested_bytes: Option<u64>,
    pub(super) usable_bytes: Option<u64>,
    pub(super) hotspots: Vec<AllocationHotspot>,
}

#[derive(Default)]
struct MemoryHotspotTotal {
    allocations: u64,
    allocated_bytes: u64,
    live_allocations: u64,
    live_bytes: u64,
}

#[derive(Default)]
struct MemoryBucketTotal<'a> {
    allocations: u64,
    allocated_bytes: u64,
    live_allocations: u64,
    live_bytes: u64,
    hotspots: HashMap<&'a [u64], MemoryHotspotTotal>,
}

impl MemorySnapshot {
    #[cfg(test)]
    pub(super) fn from_snapshot(snapshot: &crate::allocator_view::Snapshot) -> Self {
        Self::from_snapshot_with_deallocated(snapshot, &deallocated_allocations(snapshot))
    }

    pub(super) fn from_snapshot_with_deallocated(snapshot: &crate::allocator_view::Snapshot, deallocated: &HashSet<(u64, u64)>) -> Self {
        Self::from_snapshot_with_events(
            snapshot,
            deallocated,
            snapshot.callers.as_ref().map_or(&[], |callers| callers.events.as_slice()),
        )
    }

    pub(super) fn from_snapshot_with_events(
        snapshot: &crate::allocator_view::Snapshot,
        deallocated: &HashSet<(u64, u64)>,
        events: &[seismograph_rallocator::callers::Event],
    ) -> Self {
        let mut small_slices = 0;
        let mut medium_slices = 0;
        let mut bump_slices = 0;
        let mut unknown_slices = 0;
        let mut medium_allocations = MediumAllocations::default();
        for slice in snapshot.topology.iter().flat_map(|region| &region.slices) {
            match slice.kind {
                crate::allocator_topology::SliceKind::Small => small_slices += 1,
                crate::allocator_topology::SliceKind::Medium => {
                    medium_slices += 1;
                    medium_allocations.count += 1;
                    medium_allocations.requested_bytes = medium_allocations.requested_bytes.saturating_add(slice.requested_bytes);
                    medium_allocations.usable_bytes = medium_allocations.usable_bytes.saturating_add(slice.usable_bytes);
                    medium_allocations.span_slices = medium_allocations.span_slices.saturating_add(u64::from(slice.span_slices));
                    medium_allocations.largest_requested_bytes = medium_allocations.largest_requested_bytes.max(slice.requested_bytes);
                }
                crate::allocator_topology::SliceKind::Bump => bump_slices += 1,
                crate::allocator_topology::SliceKind::Unknown => unknown_slices += 1,
            }
        }
        let mut size_classes = snapshot
            .size_classes
            .iter()
            .map(|class| {
                let capacity_blocks = snapshot
                    .topology
                    .iter()
                    .flat_map(|region| &region.slices)
                    .flat_map(|slice| &slice.segments)
                    .filter(|segment| u64::from(segment.class_index) == u64::from(class.class_index))
                    .map(|segment| u64::from(segment.usable_blocks))
                    .sum();
                MemorySizeClass {
                    block_bytes: class.block_bytes,
                    live_allocations: class.live_allocations.value,
                    capacity_blocks,
                    requested_bytes: class.requested_bytes.value,
                    usable_bytes: class.usable_bytes.value,
                }
            })
            .collect::<Vec<_>>();
        size_classes.sort_unstable_by_key(|class| class.block_bytes);
        let tiers = memory_tiers(snapshot, &size_classes, &medium_allocations, deallocated, events);
        Self {
            live_bytes: snapshot.stats.live_bytes,
            peak_live_bytes: snapshot.stats.peak_live_bytes,
            peak_live_bytes_scope: snapshot.stats.peak_live_bytes_scope,
            mapped_bytes: snapshot.stats.mapped_bytes,
            allocations: snapshot.stats.allocations,
            reserved_bytes: snapshot.regions.iter().map(|region| region.reserved_bytes).sum(),
            used_slices: snapshot.regions.iter().map(|region| region.used_slices).sum(),
            free_slices: snapshot.regions.iter().map(|region| region.free_slices).sum(),
            slice_bytes: snapshot.topology.first().map_or(0, |region| region.slice_bytes),
            small_slices,
            medium_slices,
            bump_slices,
            unknown_slices,
            regions: snapshot
                .regions
                .iter()
                .map(|region| MemoryRegion {
                    index: region.index,
                    reserved_bytes: region.reserved_bytes,
                    used_slices: region.used_slices,
                    free_slices: region.free_slices,
                })
                .collect(),
            size_classes,
            medium_allocations,
            tiers,
        }
    }
}

fn memory_tiers(
    snapshot: &crate::allocator_view::Snapshot,
    size_classes: &[MemorySizeClass],
    medium_allocations: &MediumAllocations,
    deallocated: &HashSet<(u64, u64)>,
    events: &[seismograph_rallocator::callers::Event],
) -> Vec<MemoryTierData> {
    let lookups = snapshot
        .addresses
        .iter()
        .map(|lookup| (lookup.address, lookup))
        .collect::<HashMap<_, _>>();
    let mut totals = retained_memory_totals(snapshot, size_classes, deallocated, events);
    let small_current_allocations = size_classes.iter().map(|class| class.live_allocations).sum();
    let small_current_bytes = size_classes.iter().map(|class| class.requested_bytes).sum();
    MemoryTier::ALL
        .into_iter()
        .map(|kind| {
            let mut tier_totals = totals.remove(&kind).unwrap_or_default();
            let buckets: Vec<MemoryBucket> = match kind {
                MemoryTier::Small => {
                    let mut lower = 1;
                    size_classes
                        .iter()
                        .map(|class| {
                            let bucket = memory_bucket(
                                lower,
                                class.block_bytes,
                                tier_totals.remove(&class.block_bytes).unwrap_or_default(),
                                &lookups,
                            );
                            lower = class.block_bytes.saturating_add(1);
                            MemoryBucket {
                                topology_live_allocations: Some(class.live_allocations),
                                capacity_blocks: Some(class.capacity_blocks),
                                requested_bytes: Some(class.requested_bytes),
                                usable_bytes: Some(class.usable_bytes),
                                ..bucket
                            }
                        })
                        .collect()
                }
                MemoryTier::Medium | MemoryTier::Direct => tier_totals
                    .into_iter()
                    .map(|(bucket, total)| {
                        let (lower, upper) = histogram_bounds(bucket);
                        memory_bucket(lower, upper, total, &lookups)
                    })
                    .collect(),
            };
            let (current_allocations, current_bytes) = match kind {
                MemoryTier::Small => (small_current_allocations, small_current_bytes),
                MemoryTier::Medium => (medium_allocations.count, medium_allocations.requested_bytes),
                MemoryTier::Direct => {
                    let current_allocations = buckets.iter().map(|bucket| bucket.live_allocations).sum();
                    let current_bytes = buckets.iter().map(|bucket| bucket.live_bytes).sum();
                    (current_allocations, current_bytes)
                }
            };
            MemoryTierData {
                kind,
                current_allocations,
                current_bytes,
                buckets,
            }
        })
        .collect()
}

fn retained_memory_totals<'a>(
    snapshot: &crate::allocator_view::Snapshot,
    size_classes: &[MemorySizeClass],
    deallocated: &HashSet<(u64, u64)>,
    events: &'a [seismograph_rallocator::callers::Event],
) -> BTreeMap<MemoryTier, BTreeMap<u64, MemoryBucketTotal<'a>>> {
    use seismograph_rallocator::callers::EventKind;

    const MAX_SMALL_ALIGNMENT_BYTES: u64 = 4 * 1024;

    let maximum_small = size_classes.last().map_or(0, |class| class.block_bytes);
    let medium_slice = snapshot.topology.first().map_or(64 * 1024, |region| region.slice_bytes);
    let medium_region = snapshot.topology.first().map_or(1024 * 1024 * 1024, |region| region.region_bytes);
    let mut totals = BTreeMap::<MemoryTier, BTreeMap<u64, MemoryBucketTotal>>::new();
    for event in events.iter().filter(|event| event.kind == EventKind::Allocated) {
        let tier = allocation_tier(
            event.size,
            event.align,
            maximum_small,
            MAX_SMALL_ALIGNMENT_BYTES,
            medium_slice,
            medium_region,
        );
        let bucket = match tier {
            MemoryTier::Small => size_classes
                .iter()
                .find(|class| class.block_bytes >= event.size.max(event.align).max(1))
                .map_or(maximum_small, |class| class.block_bytes),
            MemoryTier::Medium | MemoryTier::Direct => u64::from(histogram_bucket(event.size)),
        };
        let total = totals.entry(tier).or_default().entry(bucket).or_default();
        total.allocations = total.allocations.saturating_add(1);
        total.allocated_bytes = total.allocated_bytes.saturating_add(event.size);
        let hotspot = total.hotspots.entry(&event.call_stack).or_default();
        hotspot.allocations = hotspot.allocations.saturating_add(1);
        hotspot.allocated_bytes = hotspot.allocated_bytes.saturating_add(event.size);
        if !deallocated.contains(&(event.thread_log_id, event.allocation_id)) {
            total.live_allocations = total.live_allocations.saturating_add(1);
            total.live_bytes = total.live_bytes.saturating_add(event.size);
            hotspot.live_allocations = hotspot.live_allocations.saturating_add(1);
            hotspot.live_bytes = hotspot.live_bytes.saturating_add(event.size);
        }
    }
    totals
}

fn allocation_tier(
    size: u64,
    align: u64,
    maximum_small: u64,
    maximum_small_alignment: u64,
    medium_slice: u64,
    medium_region: u64,
) -> MemoryTier {
    let required_small = size.max(align).max(1);
    if align <= maximum_small_alignment && required_small <= maximum_small {
        MemoryTier::Small
    } else {
        let medium_slices = size.max(1).saturating_add(medium_slice.saturating_sub(1)) / medium_slice.max(1);
        if align <= medium_slice && medium_slices.saturating_mul(medium_slice) <= medium_region {
            MemoryTier::Medium
        } else {
            MemoryTier::Direct
        }
    }
}

fn histogram_bucket(size: u64) -> u32 {
    if size == 0 { 0 } else { u64::BITS - size.leading_zeros() }
}

fn histogram_bounds(bucket: u64) -> (u64, u64) {
    if bucket == 0 {
        return (0, 0);
    }
    let shift = u32::try_from(bucket.saturating_sub(1)).unwrap_or(u32::MAX).min(63);
    let lower = 1_u64 << shift;
    let upper = if shift == 63 {
        u64::MAX
    } else {
        (1_u64 << (shift + 1)).saturating_sub(1)
    };
    (lower, upper)
}

fn memory_bucket(
    lower_bytes: u64,
    upper_bytes: u64,
    total: MemoryBucketTotal<'_>,
    lookups: &HashMap<u64, &seismograph_rallocator::callers::AddressLookup>,
) -> MemoryBucket {
    let mut hotspots = total
        .hotspots
        .into_iter()
        .map(|(stack, total)| AllocationHotspot {
            allocations: total.allocations,
            allocated_bytes: total.allocated_bytes,
            live_allocations: total.live_allocations,
            live_bytes: total.live_bytes,
            application_stack: hotspot_stack(stack, lookups, AllocationStackFilter::Application),
            complete_stack: hotspot_stack(stack, lookups, AllocationStackFilter::All),
        })
        .collect::<Vec<_>>();
    hotspots.sort_unstable_by(|left, right| {
        right
            .allocations
            .cmp(&left.allocations)
            .then_with(|| right.allocated_bytes.cmp(&left.allocated_bytes))
    });
    MemoryBucket {
        lower_bytes,
        upper_bytes,
        allocations: total.allocations,
        allocated_bytes: total.allocated_bytes,
        live_allocations: total.live_allocations,
        live_bytes: total.live_bytes,
        topology_live_allocations: None,
        capacity_blocks: None,
        requested_bytes: None,
        usable_bytes: None,
        hotspots,
    }
}

pub(super) fn deallocated_allocations(snapshot: &crate::allocator_view::Snapshot) -> HashSet<(u64, u64)> {
    // Remote frees carry the allocating owner's key but the freeing thread's sequence,
    // so the encoded order does not guarantee that allocations precede deallocations.
    snapshot
        .callers
        .iter()
        .flat_map(|callers| &callers.events)
        .filter(|event| event.kind == seismograph_rallocator::callers::EventKind::Deallocated)
        .map(|event| (event.thread_log_id, event.allocation_id))
        .collect()
}

fn allocation_records(
    events: &[seismograph_rallocator::callers::Event],
    callers: &seismograph_rallocator::callers::Callers,
    deallocated: &HashSet<(u64, u64)>,
    lookups: &HashMap<u64, &seismograph_rallocator::callers::AddressLookup>,
) -> Vec<AllocationRecord> {
    use seismograph_rallocator::callers::EventKind;

    let names = callers
        .thread_names
        .iter()
        .map(|thread| (thread.thread_id, Arc::<str>::from(thread.name.as_str())))
        .collect::<HashMap<_, _>>();
    let unnamed = Arc::<str>::from("unnamed");
    let mut record_stacks = HashMap::<&[u64], (Arc<[String]>, Arc<[String]>)>::new();
    events
        .iter()
        .map(|event| {
            let actor = names.get(&event.event_thread_id).unwrap_or(&unnamed);
            let operation = if event.kind == EventKind::Allocated { "alloc" } else { "free" };
            let correlation = if event.kind == EventKind::Deallocated && !event.allocation_recorded {
                "orphan free (no retained allocation)"
            } else if deallocated.contains(&(event.thread_log_id, event.allocation_id)) {
                "matched retained allocation/free pair (source capture)"
            } else {
                "unmatched allocation evidence (NOT proven live)"
            };
            let (application, complete) = record_stacks.entry(&event.call_stack).or_insert_with(|| {
                (
                    hotspot_stack(&event.call_stack, lookups, AllocationStackFilter::Application).into(),
                    hotspot_stack(&event.call_stack, lookups, AllocationStackFilter::All).into(),
                )
            });
            AllocationRecord {
                operation,
                correlation,
                actor_id: event.event_thread_id,
                actor_name: Arc::clone(actor),
                sequence: event.sequence,
                lifetime_id: event.allocation_id,
                origin_log: event.thread_log_id,
                address: event.address,
                size: event.size,
                alignment: event.align,
                heap_key: event.heap_id,
                application_stack: Arc::clone(application),
                complete_stack: Arc::clone(complete),
            }
        })
        .collect()
}

impl AllocationSnapshot {
    #[cfg(test)]
    pub(super) fn from_snapshot(snapshot: &crate::allocator_view::Snapshot) -> Self {
        Self::from_snapshot_with_deallocated(snapshot, &deallocated_allocations(snapshot))
    }

    pub(super) fn from_snapshot_with_deallocated(snapshot: &crate::allocator_view::Snapshot, deallocated: &HashSet<(u64, u64)>) -> Self {
        Self::from_snapshot_with_events(
            snapshot,
            deallocated,
            snapshot.callers.as_ref().map_or(&[], |callers| callers.events.as_slice()),
        )
    }

    pub(super) fn from_snapshot_with_events(
        snapshot: &crate::allocator_view::Snapshot,
        deallocated: &HashSet<(u64, u64)>,
        events: &[seismograph_rallocator::callers::Event],
    ) -> Self {
        use seismograph_rallocator::callers::EventKind;

        #[derive(Default)]
        struct Total {
            allocations: u64,
            allocated_bytes: u64,
            live_allocations: u64,
            live_bytes: u64,
        }

        let Some(callers) = &snapshot.callers else {
            return Self {
                records: Vec::new(),
                thread_count: 0,
                total_events: 0,
                retained_events: 0,
                lost_events: 0,
                hotspots: Vec::new(),
            };
        };
        let mut totals = HashMap::<&[u64], Total>::new();
        for event in events.iter().filter(|event| event.kind == EventKind::Allocated) {
            let total = totals.entry(&event.call_stack).or_default();
            total.allocations = total.allocations.saturating_add(1);
            total.allocated_bytes = total.allocated_bytes.saturating_add(event.size);
            if !deallocated.contains(&(event.thread_log_id, event.allocation_id)) {
                total.live_allocations = total.live_allocations.saturating_add(1);
                total.live_bytes = total.live_bytes.saturating_add(event.size);
            }
        }
        let lookups = snapshot
            .addresses
            .iter()
            .map(|lookup| (lookup.address, lookup))
            .collect::<HashMap<_, _>>();
        let records = allocation_records(events, callers, deallocated, &lookups);
        let mut hotspots = totals
            .into_iter()
            .map(|(stack, total)| {
                let application_stack = hotspot_stack(stack, &lookups, AllocationStackFilter::Application);
                let complete_stack = hotspot_stack(stack, &lookups, AllocationStackFilter::All);
                AllocationHotspot {
                    allocations: total.allocations,
                    allocated_bytes: total.allocated_bytes,
                    live_allocations: total.live_allocations,
                    live_bytes: total.live_bytes,
                    application_stack,
                    complete_stack,
                }
            })
            .collect::<Vec<_>>();
        hotspots.sort_unstable_by(|left, right| {
            right
                .allocations
                .cmp(&left.allocations)
                .then_with(|| right.allocated_bytes.cmp(&left.allocated_bytes))
                .then_with(|| right.live_bytes.cmp(&left.live_bytes))
        });
        Self {
            records,
            thread_count: u64::try_from(callers.threads.len()).unwrap_or(u64::MAX),
            total_events: callers.total_events,
            retained_events: u64::try_from(events.len()).unwrap_or(u64::MAX),
            lost_events: callers.lost_events,
            hotspots,
        }
    }

    pub(super) fn sorted_hotspots(&self, sort: AllocationSort, descending: bool) -> Vec<&AllocationHotspot> {
        let mut hotspots = self.hotspots.iter().collect::<Vec<_>>();
        hotspots.sort_unstable_by(|left, right| {
            let ordering = match sort {
                AllocationSort::Allocations => left.allocations.cmp(&right.allocations),
                AllocationSort::AllocatedBytes => left.allocated_bytes.cmp(&right.allocated_bytes),
                AllocationSort::AverageBytes => average_bytes(left).cmp(&average_bytes(right)),
                AllocationSort::LiveAllocations => left.live_allocations.cmp(&right.live_allocations),
                AllocationSort::LiveBytes => left.live_bytes.cmp(&right.live_bytes),
            }
            .then_with(|| left.allocated_bytes.cmp(&right.allocated_bytes))
            .then_with(|| left.allocations.cmp(&right.allocations));
            if descending { ordering.reverse() } else { ordering }
        });
        hotspots
    }
}

fn average_bytes(hotspot: &AllocationHotspot) -> u64 {
    hotspot.allocated_bytes.checked_div(hotspot.allocations).unwrap_or_default()
}

pub(super) fn hotspot_stack(
    stack: &[u64],
    lookups: &HashMap<u64, &seismograph_rallocator::callers::AddressLookup>,
    filter: AllocationStackFilter,
) -> Vec<String> {
    let frames = stack
        .iter()
        .filter_map(|address| {
            let lookup = lookups.get(address).copied();
            (filter == AllocationStackFilter::All || !is_internal_allocation_frame(lookup)).then(|| format_hotspot_frame(*address, lookup))
        })
        .collect::<Vec<_>>();
    if filter == AllocationStackFilter::Application && frames.is_empty() && !stack.is_empty() {
        vec!["No application frames after filtering".into()]
    } else {
        frames
    }
}

pub(super) fn primitive_stack(
    stack: &[u64],
    lookups: &HashMap<u64, &seismograph_rallocator::callers::AddressLookup>,
    filter: AllocationStackFilter,
) -> Vec<String> {
    let frames = stack
        .iter()
        .filter_map(|address| {
            let lookup = lookups.get(address).copied();
            (filter == AllocationStackFilter::All || !is_internal_primitive_frame(lookup)).then(|| format_hotspot_frame(*address, lookup))
        })
        .collect::<Vec<_>>();
    if filter == AllocationStackFilter::Application && frames.is_empty() && !stack.is_empty() {
        vec!["No application frames after filtering".into()]
    } else {
        frames
    }
}

fn is_internal_primitive_frame(lookup: Option<&seismograph_rallocator::callers::AddressLookup>) -> bool {
    let Some(lookup) = lookup else {
        return false;
    };
    if lookup.symbol.as_deref().is_some_and(|symbol| {
        let symbol = symbol.trim_start_matches('<');
        symbol.starts_with("performables::")
            || symbol.starts_with("seismograph::")
            || symbol.starts_with("std::")
            || symbol.starts_with("alloc::")
            || symbol.starts_with("core::")
            || symbol.starts_with("backtrace::")
    }) {
        return true;
    }
    lookup.filename.as_deref().is_some_and(|filename| {
        let filename = filename.replace('\\', "/").to_ascii_lowercase();
        filename.contains("/performables/")
            || filename.contains("/seismograph/")
            || filename.contains("/library/std/src/")
            || filename.contains("/library/alloc/src/")
            || filename.contains("/library/core/src/")
    })
}

fn is_internal_allocation_frame(lookup: Option<&seismograph_rallocator::callers::AddressLookup>) -> bool {
    let Some(lookup) = lookup else {
        return false;
    };
    if lookup.symbol.as_deref().is_some_and(|symbol| {
        let symbol = symbol.trim_start_matches('<');
        symbol.starts_with("rallocator::")
            || symbol.starts_with("seismograph::")
            || symbol.starts_with("seismograph_rallocator::")
            || symbol.starts_with("std::")
            || symbol.starts_with("alloc::")
            || symbol.starts_with("core::")
    }) {
        return true;
    }
    lookup.filename.as_deref().is_some_and(|filename| {
        let filename = filename.replace('\\', "/").to_ascii_lowercase();
        filename.contains("/rallocator/")
            || filename.contains("/seismograph/")
            || filename.contains("/seismograph_rallocator/")
            || filename.contains("/library/std/src/")
            || filename.contains("/library/alloc/src/")
            || filename.contains("/library/core/src/")
    })
}

fn format_hotspot_frame(address: u64, lookup: Option<&seismograph_rallocator::callers::AddressLookup>) -> String {
    let Some(lookup) = lookup else {
        return format!("0x{address:016x}");
    };
    let symbol = lookup.symbol.as_deref().unwrap_or("unknown");
    let Some(filename) = &lookup.filename else {
        return symbol.to_owned();
    };
    let filename = filename.rsplit(['/', '\\']).next().unwrap_or(filename);
    match lookup.line {
        Some(line) => format!("{symbol} ({filename}:{line})"),
        None => format!("{symbol} ({filename})"),
    }
}

#[cfg(test)]
mod tests {
    use seismograph::recorder::RecordingPolicies;
    use seismograph::recorder::event::{
        Address as RuntimeAddress, Event as RuntimeEvent, EventClock, EventKind as RuntimeEventKind, EventPayload, EventSequence,
        EventTimestamp, Events, NumericEvent, ObjectId,
    };
    use seismograph::recorder::io::{IoEvent, IoOperationId, IoOutcome, IoResourceId, IoResourceKind};
    use seismograph::recorder::runtime::{RuntimeEvent as RuntimeEventPayload, RuntimeId, WorkerId};
    use seismograph::recorder::thread::{ThreadId, ThreadLog};
    use seismograph_rallocator::callers::{
        AddressLookup, AddressLookupFields, Callers, CallersFields, Event, EventFields, EventKind, HeapKind,
    };

    use super::super::runtime_timeline::{ExecutionMetrics, Interval, median_nanos};
    use super::*;

    #[test]
    fn model_enum_navigation_and_labels_cover_every_variant() {
        assert_eq!(
            PrimitiveKind::ALL.map(|kind| (kind.label(), kind.operations().len())),
            [
                ("Arc", 5),
                ("Mutex", 6),
                ("RwLock", 9),
                ("Barrier", 3),
                ("Condvar", 3),
                ("OnceLock / LazyLock", 3),
                ("Channel", 6),
            ]
        );
        assert_eq!(
            [MemoryTier::Small, MemoryTier::Medium, MemoryTier::Direct,].map(|tier| (tier.next(), tier.previous())),
            [
                (MemoryTier::Medium, MemoryTier::Direct),
                (MemoryTier::Direct, MemoryTier::Small),
                (MemoryTier::Small, MemoryTier::Medium),
            ]
        );
        assert_eq!(
            [
                AllocationSort::Allocations,
                AllocationSort::AllocatedBytes,
                AllocationSort::AverageBytes,
                AllocationSort::LiveAllocations,
                AllocationSort::LiveBytes,
            ]
            .map(|sort| (sort.next(), sort.previous())),
            [
                (AllocationSort::AllocatedBytes, AllocationSort::LiveBytes),
                (AllocationSort::AverageBytes, AllocationSort::Allocations),
                (AllocationSort::LiveAllocations, AllocationSort::AllocatedBytes),
                (AllocationSort::LiveBytes, AllocationSort::AverageBytes),
                (AllocationSort::Allocations, AllocationSort::LiveAllocations),
            ]
        );
        assert_eq!(
            [
                PrimitiveSort::Events,
                PrimitiveSort::Objects,
                PrimitiveSort::Threads,
                PrimitiveSort::Hotspots,
            ]
            .map(|sort| (sort.next(), sort.previous())),
            [
                (PrimitiveSort::Objects, PrimitiveSort::Hotspots),
                (PrimitiveSort::Threads, PrimitiveSort::Events),
                (PrimitiveSort::Hotspots, PrimitiveSort::Objects),
                (PrimitiveSort::Events, PrimitiveSort::Threads),
            ]
        );
        assert_eq!(
            [
                RuntimeTaskSort::Task,
                RuntimeTaskSort::FutureSize,
                RuntimeTaskSort::Polls,
                RuntimeTaskSort::Executing,
                RuntimeTaskSort::MedianPoll,
                RuntimeTaskSort::MaximumPoll,
            ]
            .map(|sort| (sort.next(), sort.previous(), sort.label())),
            [
                (RuntimeTaskSort::FutureSize, RuntimeTaskSort::MaximumPoll, "task"),
                (RuntimeTaskSort::Polls, RuntimeTaskSort::Task, "future bytes"),
                (RuntimeTaskSort::Executing, RuntimeTaskSort::FutureSize, "polls"),
                (RuntimeTaskSort::MedianPoll, RuntimeTaskSort::Polls, "observed execution"),
                (RuntimeTaskSort::MaximumPoll, RuntimeTaskSort::Executing, "median poll"),
                (RuntimeTaskSort::Task, RuntimeTaskSort::MedianPoll, "maximum poll"),
            ]
        );
        assert_eq!(
            (AllocationStackFilter::Application.toggle(), AllocationStackFilter::All.toggle()),
            (AllocationStackFilter::All, AllocationStackFilter::Application)
        );
    }

    #[test]
    fn io_and_cache_labels_cover_every_supported_variant() {
        assert_eq!([IoOperationKind::Read.label(), IoOperationKind::Write.label()], ["Read", "Write"]);
        assert_eq!(
            CACHE_EVENT_KINDS.map(cache_event_label),
            [
                "Hit",
                "Miss",
                "Expired",
                "Get error",
                "Inserted",
                "Insert rejected",
                "Insert error",
                "Invalidated",
                "Invalidate error",
                "Cleared",
                "Clear error",
                "Refresh hit",
                "Refresh miss",
                "Refresh error",
                "Evicted",
                "Compute succeeded",
                "Compute failed",
                "Compute returned none",
                "Promotion accepted",
                "Promotion rejected",
                "Promotion failed",
                "Refresh suppressed",
            ]
        );
        assert_eq!(cache_event_label(RuntimeEventKind::MutexAccess), "Unknown");
    }

    #[test]
    fn primitive_operation_metadata_covers_every_variant() {
        let operations = [
            PrimitiveOperationKind::ArcCreate,
            PrimitiveOperationKind::ArcClone,
            PrimitiveOperationKind::ArcDeref,
            PrimitiveOperationKind::ArcDrop,
            PrimitiveOperationKind::ArcRelocate,
            PrimitiveOperationKind::MutexAccess,
            PrimitiveOperationKind::MutexContention,
            PrimitiveOperationKind::MutexRelease,
            PrimitiveOperationKind::RwLockReadAccess,
            PrimitiveOperationKind::RwLockReadContention,
            PrimitiveOperationKind::RwLockReadRelease,
            PrimitiveOperationKind::RwLockWriteAccess,
            PrimitiveOperationKind::RwLockWriteContention,
            PrimitiveOperationKind::RwLockWriteRelease,
            PrimitiveOperationKind::BarrierAccess,
            PrimitiveOperationKind::BarrierContention,
            PrimitiveOperationKind::BarrierRelease,
            PrimitiveOperationKind::CondvarAccess,
            PrimitiveOperationKind::CondvarContention,
            PrimitiveOperationKind::CondvarNotify,
            PrimitiveOperationKind::OnceAccess,
            PrimitiveOperationKind::OnceContention,
            PrimitiveOperationKind::OnceInitialize,
            PrimitiveOperationKind::ChannelSend,
            PrimitiveOperationKind::ChannelSendContention,
            PrimitiveOperationKind::ChannelReceive,
            PrimitiveOperationKind::ChannelReceiveContention,
            PrimitiveOperationKind::ChannelClose,
            PrimitiveOperationKind::ChannelHighWatermark,
            PrimitiveOperationKind::LockPoisoned,
            PrimitiveOperationKind::LockPoisonObserved,
            PrimitiveOperationKind::LockPoisonCleared,
        ];

        assert_eq!(
            operations.map(PrimitiveOperationKind::label),
            [
                "Create",
                "Clone",
                "Deref",
                "Final drop",
                "Relocate",
                "Acquisition",
                "Contention",
                "Release",
                "Read acquisition",
                "Read contention",
                "Read release",
                "Write acquisition",
                "Write contention",
                "Write release",
                "Completed wait",
                "Blocked wait",
                "Generation release",
                "Completed wait",
                "Blocked wait",
                "Notification",
                "Access",
                "Contention",
                "Initialization",
                "Send",
                "Send contention",
                "Receive",
                "Receive wait (empty)",
                "Close",
                "High watermark",
                "Poisoned",
                "Poison observed",
                "Poison cleared",
            ]
        );
        assert_eq!(
            (
                PrimitiveOperationKind::MutexContention.is_contention(),
                PrimitiveOperationKind::ArcClone.is_contention(),
                PrimitiveOperationKind::LockPoisoned.is_lock_poison(),
                PrimitiveOperationKind::ArcClone.is_lock_poison(),
            ),
            (true, false, true, false)
        );
        for kind in PrimitiveKind::ALL {
            for operation in operations {
                let _ = kind.identifies(operation.event_kind());
            }
        }
    }

    #[test]
    fn thread_operation_metadata_covers_every_variant() {
        for kind in ThreadOperationKind::ALL {
            assert!(!kind.label().is_empty());
            assert!(!kind.relationship_label().is_empty());
            for related in ThreadOperationKind::ALL {
                let _ = kind.is_related(related.event_kind());
            }
        }
        assert_eq!(ThreadOperationKind::Allocation.label(), "Allocation");
        assert_eq!(ThreadOperationKind::ChannelHighWatermark.label(), "Channel high watermark");
        assert_eq!(
            ThreadOperationKind::Allocation.relationship_label(),
            "Threads that deallocated these allocations"
        );
        assert_eq!(
            ThreadOperationKind::MutexAccess.relationship_label(),
            "Threads (including self) observed on the same Mutex objects"
        );
        assert_eq!(
            (
                ThreadOperationKind::MutexContention.is_contention(),
                ThreadOperationKind::ArcClone.is_contention(),
                ThreadOperationKind::Allocation.is_allocation(),
                ThreadOperationKind::ArcClone.is_allocation(),
                ThreadOperationKind::ArcClone.is_related(RuntimeEventKind::ArcDeref),
                ThreadOperationKind::ArcClone.is_related(RuntimeEventKind::MutexAccess),
            ),
            (true, false, true, false, true, false)
        );
    }

    #[test]
    fn identical_thread_stacks_are_counted() {
        let events = [
            RuntimeEvent {
                thread_id: ThreadId::new(1),
                sequence: EventSequence::new(1),
                timestamp: EventTimestamp::from_ticks(1),
                kind: RuntimeEventKind::ArcClone,
                payload: EventPayload::Object(ObjectId::new(7)),
                call_stack: vec![RuntimeAddress::new(0x1000)],
            },
            RuntimeEvent {
                thread_id: ThreadId::new(1),
                sequence: EventSequence::new(2),
                timestamp: EventTimestamp::from_ticks(2),
                kind: RuntimeEventKind::ArcClone,
                payload: EventPayload::Object(ObjectId::new(7)),
                call_stack: vec![RuntimeAddress::new(0x1000)],
            },
        ];

        assert_eq!(
            thread_stacks(events.iter(), ThreadOperationKind::ArcClone, &mut ThreadStacks::new(&[])),
            ThreadStackSet::One(ThreadStack {
                count: 2,
                frames: Arc::new(ThreadFrames {
                    application: vec!["0x0000000000001000".into()],
                    complete: vec!["0x0000000000001000".into()],
                }),
            })
        );
    }

    #[test]
    fn empty_thread_events_have_no_representative_stack() {
        let stacks = thread_stacks(std::iter::empty(), ThreadOperationKind::ArcClone, &mut ThreadStacks::new(&[]));
        assert_eq!((stacks.first(), &stacks), (None, &ThreadStackSet::Empty));
    }

    #[test]
    fn compact_stack_sets_preserve_the_first_stack_and_order() {
        let stack = |count| ThreadStack {
            count,
            frames: Arc::new(ThreadFrames {
                application: vec!["application::run".into()],
                complete: vec!["application::run".into()],
            }),
        };
        let first = stack(3);
        let second = stack(1);
        let empty = ThreadStackSet::from(Vec::new());
        let one = ThreadStackSet::from(vec![first.clone()]);
        let many = ThreadStackSet::from(vec![first.clone(), second.clone()]);
        assert_eq!(
            (empty, one, many.first(), &many),
            (
                ThreadStackSet::Empty,
                ThreadStackSet::One(first.clone()),
                Some(&first),
                &ThreadStackSet::Many(vec![first.clone(), second]),
            ),
        );
    }

    #[test]
    fn unrelated_operations_on_the_same_object_do_not_link_threads() {
        let events = Events {
            events: [RuntimeEventKind::ArcClone, RuntimeEventKind::MutexAccess]
                .into_iter()
                .enumerate()
                .map(|(index, kind)| RuntimeEvent {
                    thread_id: ThreadId::new(u64::try_from(index).unwrap() + 1),
                    sequence: EventSequence::new(1),
                    timestamp: EventTimestamp::from_ticks(1),
                    kind,
                    payload: EventPayload::Object(ObjectId::new(7)),
                    call_stack: Vec::new(),
                })
                .collect(),
            ..Default::default()
        };
        let snapshot = ThreadSnapshot::from_events(&events, &[]);
        let operations = snapshot
            .threads
            .iter()
            .flat_map(|thread| &thread.operations)
            .filter(|operation| operation.events > 0)
            .map(|operation| (operation.events, operation.objects, operation.participants.len()))
            .collect::<Vec<_>>();
        assert_eq!(operations, [(1, 1, 1), (1, 1, 1)]);
        for thread in &snapshot.threads {
            assert!(
                thread
                    .operations
                    .iter()
                    .flat_map(|operation| &operation.participants)
                    .all(|participant| { participant.thread_id == thread.thread_id })
            );
        }
    }

    #[test]
    fn stack_formatting_filters_internal_frames_and_handles_missing_metadata() {
        let lookups = [
            AddressLookup::from_fields(AddressLookupFields {
                address: 1,
                symbol: Some("<std::alloc::allocate>".into()),
                filename: None,
                line: None,
                column: None,
            }),
            AddressLookup::from_fields(AddressLookupFields {
                address: 2,
                symbol: Some("app::run".into()),
                filename: Some(r"C:\src\app.rs".into()),
                line: Some(7),
                column: None,
            }),
            AddressLookup::from_fields(AddressLookupFields {
                address: 3,
                symbol: None,
                filename: Some(r"C:\src\file.rs".into()),
                line: None,
                column: None,
            }),
        ];
        let lookups = lookups.iter().map(|lookup| (lookup.address, lookup)).collect::<HashMap<_, _>>();

        assert_eq!(
            (
                hotspot_stack(&[1, 2], &lookups, AllocationStackFilter::Application),
                primitive_stack(&[1], &lookups, AllocationStackFilter::Application),
                hotspot_stack(&[1], &lookups, AllocationStackFilter::Application),
                hotspot_stack(&[4], &lookups, AllocationStackFilter::Application),
                format_hotspot_frame(3, lookups.get(&3).copied()),
                format_hotspot_frame(4, None),
            ),
            (
                vec!["app::run (app.rs:7)".to_owned()],
                vec!["No application frames after filtering".to_owned()],
                vec!["No application frames after filtering".to_owned()],
                vec!["0x0000000000000004".to_owned()],
                "unknown (file.rs)".to_owned(),
                "0x0000000000000004".to_owned(),
            )
        );
    }

    #[test]
    #[cfg_attr(miri, ignore = "quadratic differential stress test requires native execution")]
    fn thread_object_index_matches_reference_for_every_operation_and_event_order() {
        let mut events = Vec::new();
        for object in 1..=16 {
            for thread in 1..=4 {
                for (index, kind) in ThreadOperationKind::ALL.into_iter().enumerate() {
                    for repetition in 0..=(index % 3) {
                        events.push(RuntimeEvent {
                            thread_id: ThreadId::new(thread),
                            sequence: EventSequence::new(u64::try_from(events.len()).unwrap()),
                            timestamp: EventTimestamp::from_ticks(1),
                            kind: kind.event_kind(),
                            payload: EventPayload::Object(ObjectId::new(object)),
                            call_stack: vec![RuntimeAddress::new(0x1000 + u64::try_from(repetition % 2).unwrap())],
                        });
                    }
                }
            }
        }
        let mut decoded = Events {
            events,
            threads: vec![ThreadLog {
                thread_id: ThreadId::new(99),
                name: "empty thread".into(),
                total_events: 12,
                lost_events: 5,
            }],
            ..Default::default()
        };
        let canonical = |mut snapshot: ThreadSnapshot| {
            for object in snapshot
                .threads
                .iter_mut()
                .flat_map(|thread| &mut thread.operations)
                .flat_map(|operation| &mut operation.participants)
                .flat_map(|participant| &mut participant.objects)
            {
                for stacks in [&mut object.selected_stacks, &mut object.related_stacks] {
                    if let ThreadStackSet::Many(stacks) = stacks {
                        stacks.sort_unstable_by(|left, right| {
                            left.count
                                .cmp(&right.count)
                                .then_with(|| left.frames.complete.cmp(&right.frames.complete))
                        });
                    }
                }
            }
            snapshot
        };
        for _ in 0..2 {
            assert_eq!(
                canonical(ThreadSnapshot::from_events(&decoded, &[])),
                canonical(ThreadSnapshot::from_events_reference(&decoded, &[])),
            );
            decoded.events.reverse();
        }
    }

    #[test]
    fn thread_summary_shares_formatted_stacks_across_objects() {
        let object_count = if cfg!(miri) { 32 } else { 2_000 };
        let events = Events {
            events: (1..=object_count)
                .flat_map(|object| {
                    [1, 2].map(|thread| RuntimeEvent {
                        thread_id: ThreadId::new(thread),
                        sequence: EventSequence::new(object),
                        timestamp: EventTimestamp::from_ticks(object),
                        kind: RuntimeEventKind::ArcClone,
                        payload: EventPayload::Object(ObjectId::new(object)),
                        call_stack: vec![RuntimeAddress::new(0x1000)],
                    })
                })
                .collect(),
            ..Default::default()
        };
        let snapshot = ThreadSnapshot::from_events(&events, &[]);
        for thread in &snapshot.threads {
            let operation = thread
                .operations
                .iter()
                .find(|operation| operation.kind == ThreadOperationKind::ArcClone)
                .unwrap();
            let objects = &operation.participants[0].objects;
            assert_eq!(
                (operation.events, operation.objects, objects.len()),
                (object_count, object_count, usize::try_from(object_count).unwrap())
            );
            let first = objects.first().unwrap().selected_stack().unwrap();
            let last = objects.last().unwrap().related_stack().unwrap();
            assert!(Arc::ptr_eq(&first.frames, &last.frames));
        }
    }

    #[test]
    fn allocation_routing_and_histogram_bounds_cover_edges() {
        assert_eq!(
            (
                allocation_tier(1, 1, 64, 4_096, 65_536, 1 << 30),
                allocation_tier(65, 1, 64, 4_096, 65_536, 1 << 30),
                allocation_tier(1, 65_537, 64, 4_096, 65_536, 1 << 30),
                histogram_bucket(0),
                histogram_bucket(1),
                histogram_bounds(0),
                histogram_bounds(1),
                histogram_bounds(64),
            ),
            (
                MemoryTier::Small,
                MemoryTier::Medium,
                MemoryTier::Direct,
                0,
                1,
                (0, 0),
                (1, 1),
                (1 << 63, u64::MAX),
            )
        );
    }

    fn runtime_task(task_id: u64) -> RuntimeTaskSummary {
        RuntimeTaskSummary {
            task_id,
            runtime_id: 1,
            state: "Unknown".into(),
            metrics: ExecutionMetrics {
                poll_count: task_id,
                median_poll_nanos: Some(task_id * 2),
                max_poll_nanos: Some(task_id * 3),
                executing_fraction: Some(if task_id == 1 { 0.25 } else { 0.5 }),
                ..ExecutionMetrics::default()
            },
            ..RuntimeTaskSummary::default()
        }
    }

    #[test]
    #[expect(clippy::too_many_lines, reason = "one table-driven test covers every sort key and both directions")]
    fn sorting_helpers_cover_every_column_and_direction() {
        let worker = RuntimeWorkerSummary {
            runtime_id: 1,
            runtime_name: String::new(),
            worker_id: Some(1),
            role: String::new(),
            state: String::new(),
            thread_id: None,
            current_task: None,
            tasks: vec![runtime_task(1), runtime_task(2)],
            ..RuntimeWorkerSummary::default()
        };
        for sort in [
            RuntimeTaskSort::Task,
            RuntimeTaskSort::Polls,
            RuntimeTaskSort::Executing,
            RuntimeTaskSort::MedianPoll,
            RuntimeTaskSort::MaximumPoll,
        ] {
            assert_eq!(
                worker
                    .sorted_tasks(sort, false)
                    .into_iter()
                    .map(|task| task.task_id)
                    .collect::<Vec<_>>(),
                vec![1, 2]
            );
            assert_eq!(
                worker
                    .sorted_tasks(sort, true)
                    .into_iter()
                    .map(|task| task.task_id)
                    .collect::<Vec<_>>(),
                vec![2, 1]
            );
        }

        let operations = PrimitiveGroup {
            kind: PrimitiveKind::Arc,
            events: 0,
            objects: 0,
            contentions: 0,
            operations: vec![
                PrimitiveOperation {
                    kind: PrimitiveOperationKind::ArcClone,
                    events: 1,
                    objects: 2,
                    threads: 3,
                    hotspots: vec![PrimitiveHotspot {
                        count: 1,
                        application_stack: Vec::new(),
                        complete_stack: Vec::new(),
                    }],
                },
                PrimitiveOperation {
                    kind: PrimitiveOperationKind::ArcDeref,
                    events: 2,
                    objects: 3,
                    threads: 4,
                    hotspots: vec![
                        PrimitiveHotspot {
                            count: 1,
                            application_stack: Vec::new(),
                            complete_stack: Vec::new(),
                        },
                        PrimitiveHotspot {
                            count: 1,
                            application_stack: Vec::new(),
                            complete_stack: Vec::new(),
                        },
                    ],
                },
            ],
        };
        for sort in [
            PrimitiveSort::Events,
            PrimitiveSort::Objects,
            PrimitiveSort::Threads,
            PrimitiveSort::Hotspots,
        ] {
            assert_eq!(operations.sorted_operations(sort, false)[0].kind, PrimitiveOperationKind::ArcClone);
            assert_eq!(operations.sorted_operations(sort, true)[0].kind, PrimitiveOperationKind::ArcDeref);
        }

        let hotspot = |allocations, allocated_bytes, live_allocations, live_bytes| AllocationHotspot {
            allocations,
            allocated_bytes,
            live_allocations,
            live_bytes,
            application_stack: Vec::new(),
            complete_stack: Vec::new(),
        };
        let allocations = AllocationSnapshot {
            records: Vec::new(),
            thread_count: 0,
            total_events: 0,
            retained_events: 0,
            lost_events: 0,
            hotspots: vec![hotspot(1, 10, 1, 10), hotspot(2, 40, 2, 40)],
        };
        for sort in [
            AllocationSort::Allocations,
            AllocationSort::AllocatedBytes,
            AllocationSort::AverageBytes,
            AllocationSort::LiveAllocations,
            AllocationSort::LiveBytes,
        ] {
            assert_eq!(allocations.sorted_hotspots(sort, false)[0].allocations, 1);
            assert_eq!(allocations.sorted_hotspots(sort, true)[0].allocations, 2);
        }
        assert_eq!((average_bytes(&hotspot(0, 10, 0, 0)), average_bytes(&hotspot(4, 10, 0, 0))), (0, 2));
    }

    #[test]
    fn snapshot_accessors_cover_empty_and_populated_stacks() {
        let allocation = AllocationHotspot {
            allocations: 1,
            allocated_bytes: 1,
            live_allocations: 1,
            live_bytes: 1,
            application_stack: vec!["app".into()],
            complete_stack: vec!["internal".into(), "app".into()],
        };
        let primitive = PrimitiveHotspot {
            count: 1,
            application_stack: Vec::new(),
            complete_stack: vec!["internal".into()],
        };
        let object = ThreadObject {
            object_id: 1,
            selected_events: 2,
            related_events: 3,
            selected_stacks: ThreadStackSet::One(ThreadStack {
                count: 1,
                frames: Arc::new(ThreadFrames {
                    application: vec!["selected".into()],
                    complete: vec!["selected-all".into()],
                }),
            }),
            related_stacks: ThreadStackSet::Empty,
        };

        assert_eq!(
            (
                allocation.location(AllocationStackFilter::Application),
                allocation.stack(AllocationStackFilter::All),
                primitive.location(AllocationStackFilter::Application),
                primitive.stack(AllocationStackFilter::All),
                object.hotness(),
                object.selected_stack().unwrap().stack(AllocationStackFilter::Application),
                object.related_stack(),
            ),
            (
                "app",
                &["internal".to_owned(), "app".to_owned()][..],
                "Backtraces disabled",
                &["internal".to_owned()][..],
                5,
                &["selected".to_owned()][..],
                None,
            )
        );
    }

    #[test]
    fn retained_memory_totals_and_task_ids_handle_missing_inputs() {
        let snapshot = crate::allocator_view::Snapshot::new(crate::allocator_view::Version::new(1, 0, 0));
        assert!(retained_memory_totals(&snapshot, &[], &HashSet::new(), &[]).is_empty());
        assert_eq!(
            (
                runtime_task_id(RuntimeEventKind::TaskSpawned, 1, 0),
                runtime_task_id(RuntimeEventKind::TaskPollFinished, 2, 0),
                runtime_task_id(RuntimeEventKind::TaskReady, 8, 9),
                runtime_task_id(RuntimeEventKind::TransferStarted, 0, 3),
                runtime_task_id(RuntimeEventKind::InstanceRelocated, 0, 4),
                runtime_task_id(RuntimeEventKind::TransferFinished, 0, 5),
                runtime_task_id(RuntimeEventKind::ArcClone, 6, 7),
                runtime_task_id(RuntimeEventKind::TaskCanceled, 0, 0),
            ),
            (Some(1), Some(2), Some(8), Some(3), Some(4), Some(5), None, None)
        );
    }

    #[test]
    fn median_handles_skewed_odd_samples() {
        assert_eq!(median_nanos(&mut [100, 1, 2]), Some(2));
    }

    #[test]
    fn median_rounds_even_samples_down() {
        assert_eq!(median_nanos(&mut [100, 4, 1, 3]), Some(3));
    }

    #[test]
    fn median_distinguishes_absent_and_zero_samples() {
        assert_eq!(
            [median_nanos(&mut []), median_nanos(&mut [0]), median_nanos(&mut [0, 0])],
            [None, Some(0), Some(0)]
        );
    }

    #[test]
    fn median_handles_maximum_samples_without_overflow() {
        assert_eq!(
            [
                median_nanos(&mut [u64::MAX]),
                median_nanos(&mut [u64::MAX, u64::MAX]),
                median_nanos(&mut [u64::MAX, u64::MAX - 1]),
                median_nanos(&mut [u64::MAX, 0]),
            ],
            [Some(u64::MAX), Some(u64::MAX), Some(u64::MAX - 1), Some(u64::MAX / 2)]
        );
    }

    #[test]
    fn median_sorts_keep_missing_samples_last_and_break_ties_by_task_id() {
        let worker = RuntimeWorkerSummary {
            tasks: [(6, None), (5, Some(10)), (4, Some(10)), (3, None), (2, Some(0)), (1, Some(20))]
                .into_iter()
                .map(|(task_id, median)| RuntimeTaskSummary {
                    metrics: ExecutionMetrics {
                        median_poll_nanos: median,
                        max_poll_nanos: median,
                        ..ExecutionMetrics::default()
                    },
                    ..runtime_task(task_id)
                })
                .collect(),
            ..RuntimeWorkerSummary::default()
        };
        assert_eq!(
            [
                (RuntimeTaskSort::MedianPoll, false),
                (RuntimeTaskSort::MedianPoll, true),
                (RuntimeTaskSort::MaximumPoll, false),
                (RuntimeTaskSort::MaximumPoll, true),
            ]
            .map(|(sort, descending)| {
                worker
                    .sorted_tasks(sort, descending)
                    .into_iter()
                    .map(|task| task.task_id)
                    .collect::<Vec<_>>()
            }),
            [
                vec![2, 4, 5, 1, 3, 6],
                vec![1, 4, 5, 2, 3, 6],
                vec![2, 4, 5, 1, 3, 6],
                vec![1, 4, 5, 2, 3, 6],
            ]
        );
    }

    #[test]
    fn runtime_execution_metrics_handle_absent_zero_and_populated_samples() {
        let mut empty = ExecutionMetrics::default();
        let mut zero = ExecutionMetrics {
            poll_samples: vec![0],
            ..ExecutionMetrics::default()
        };
        let mut populated = ExecutionMetrics {
            poll_samples: vec![3, 100, 5],
            ..ExecutionMetrics::default()
        };
        for metrics in [&mut empty, &mut zero, &mut populated] {
            metrics.finish(None);
        }
        assert_eq!(
            [empty, zero, populated].map(|metrics| (metrics.poll_count, metrics.median_poll_nanos, metrics.max_poll_nanos)),
            [(0, None, None), (1, Some(0), Some(0)), (3, Some(5), Some(100))]
        );
    }

    #[test]
    fn workerless_lifecycle_events_remain_visible_without_a_runtime_source() {
        let events = [RuntimeEventKind::TaskSpawned, RuntimeEventKind::TaskCanceled]
            .into_iter()
            .enumerate()
            .map(|(index, kind)| RuntimeEvent {
                thread_id: ThreadId::new(1),
                sequence: EventSequence::new(u64::try_from(index).unwrap()),
                timestamp: EventTimestamp::from_ticks(u64::try_from(index).unwrap() + 1),
                kind,
                payload: EventPayload::Runtime(RuntimeEventPayload {
                    runtime_id: RuntimeId::from_raw(1).unwrap(),
                    worker_id: None,
                    subject_id: 10,
                    related_id: 0,
                    value_0: 42,
                    value_1: 0,
                }),
                call_stack: Vec::new(),
            })
            .collect();
        let snapshot = RuntimeMonitorSnapshot::from_events(
            &Events {
                events,
                ..Events::default()
            },
            None,
            &[],
        );
        let group = &snapshot.workers[0];
        assert_eq!(
            (
                snapshot.runtime_events,
                snapshot.source_present,
                group.runtime_id,
                group.worker_id,
                group
                    .tasks
                    .iter()
                    .map(|task| (task.task_id, task.state.as_str(), task.activity.state.as_str()))
                    .collect::<Vec<_>>(),
            ),
            (2, false, 1, None, vec![(10, "Canceled", "Canceled")])
        );
    }

    #[test]
    fn retained_spawn_metadata_does_not_infer_current_activity() {
        let event = RuntimeEvent {
            thread_id: ThreadId::new(1),
            sequence: EventSequence::new(1),
            timestamp: EventTimestamp::from_ticks(1),
            kind: RuntimeEventKind::TaskSpawned,
            payload: EventPayload::Runtime(RuntimeEventPayload {
                runtime_id: RuntimeId::from_raw(1).unwrap(),
                worker_id: None,
                subject_id: 1,
                related_id: 0,
                value_0: 0,
                value_1: 0,
            }),
            call_stack: Vec::new(),
        };

        let snapshot = RuntimeMonitorSnapshot::from_events(
            &Events {
                events: vec![event],
                ..Events::default()
            },
            None,
            &[],
        );
        let task = &snapshot.workers[0].tasks[0];
        assert_eq!(
            (
                task.task_id,
                task.state.as_str(),
                task.activity.state.as_str(),
                task.metrics.poll_count
            ),
            (1, "Unknown", "Unknown", 0)
        );
    }

    #[test]
    fn allocation_snapshot_without_callers_and_unmatched_deallocation_is_empty() {
        let mut snapshot = crate::allocator_view::Snapshot::new(crate::allocator_view::Version::new(1, 0, 0));
        assert_eq!(
            AllocationSnapshot::from_snapshot(&snapshot),
            AllocationSnapshot {
                records: Vec::new(),
                thread_count: 0,
                total_events: 0,
                retained_events: 0,
                lost_events: 0,
                hotspots: Vec::new(),
            }
        );

        let mut event = Event::default();
        event.kind = EventKind::Deallocated;
        snapshot.callers = Some(Callers::from_fields(CallersFields {
            session_id: 0,
            total_events: 1,
            lost_events: 0,
            threads: Vec::new(),
            events: vec![event],
            thread_names: Vec::new(),
        }));
        assert!(AllocationSnapshot::from_snapshot(&snapshot).hotspots.is_empty());
    }

    #[test]
    fn memory_bucket_ranks_equal_counts_by_allocated_bytes() {
        let mut total = MemoryBucketTotal::default();
        total.hotspots.insert(
            &[1],
            MemoryHotspotTotal {
                allocations: 1,
                allocated_bytes: 10,
                ..MemoryHotspotTotal::default()
            },
        );
        total.hotspots.insert(
            &[2],
            MemoryHotspotTotal {
                allocations: 1,
                allocated_bytes: 20,
                ..MemoryHotspotTotal::default()
            },
        );

        assert_eq!(memory_bucket(1, 2, total, &HashMap::new()).hotspots[0].allocated_bytes, 20);
    }

    #[test]
    fn runtime_monitor_applies_lifecycle_and_transfer_events() {
        let event = |sequence, kind, subject_id, related_id| RuntimeEvent {
            thread_id: ThreadId::new(1),
            sequence: EventSequence::new(sequence),
            timestamp: EventTimestamp::from_ticks(sequence),
            kind,
            payload: EventPayload::Runtime(RuntimeEventPayload {
                runtime_id: RuntimeId::from_raw(1).unwrap(),
                worker_id: Some(WorkerId::from_raw(1).unwrap()),
                subject_id,
                related_id,
                value_0: 0,
                value_1: 0,
            }),
            call_stack: Vec::new(),
        };
        let events = vec![
            event(1, RuntimeEventKind::TaskSpawned, 1, 0),
            event(2, RuntimeEventKind::TaskEnqueued, 1, 0),
            event(3, RuntimeEventKind::TaskMaterialized, 1, 0),
            event(4, RuntimeEventKind::TransferStarted, 0, 1),
            event(5, RuntimeEventKind::InstanceRelocated, 0, 1),
            event(6, RuntimeEventKind::TransferFinished, 0, 1),
            event(7, RuntimeEventKind::TaskCanceled, 1, 0),
            event(8, RuntimeEventKind::TaskSpawned, 2, 0),
            event(9, RuntimeEventKind::TaskPanicked, 2, 0),
            event(10, RuntimeEventKind::TaskSpawned, 3, 0),
            event(11, RuntimeEventKind::TaskCompleted, 3, 0),
            event(12, RuntimeEventKind::RuntimeCreated, 0, 0),
            RuntimeEvent {
                thread_id: ThreadId::new(1),
                sequence: EventSequence::new(13),
                timestamp: EventTimestamp::from_ticks(13),
                kind: RuntimeEventKind::ArcClone,
                payload: EventPayload::Object(ObjectId::new(9)),
                call_stack: Vec::new(),
            },
        ];

        let snapshot = RuntimeMonitorSnapshot::from_events(
            &Events {
                clock: EventClock::Unspecified,
                total_events: 13,
                lost_events: 0,
                recording: RecordingPolicies::default(),
                threads: Vec::new(),
                events,
            },
            None,
            &[],
        );
        let tasks = &snapshot.workers[0].tasks;

        assert_eq!(
            tasks
                .iter()
                .map(|task| {
                    (
                        task.task_id,
                        task.state.as_str(),
                        task.worker_ids.as_slice(),
                        task.metrics.poll_count,
                    )
                })
                .collect::<Vec<_>>(),
            vec![
                (1, "Canceled", &[1][..], 0),
                (2, "Panicked", &[1][..], 0),
                (3, "Completed", &[1][..], 0)
            ]
        );
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "the single assertion compares the complete memory snapshot fixture"
    )]
    fn memory_snapshot_summarizes_regions_and_slices() {
        let mut snapshot = crate::allocator_view::Snapshot::new(crate::allocator_view::Version::new(0, 1, 0));
        snapshot.stats.live_bytes = 10;
        snapshot.stats.peak_live_bytes = 20;
        snapshot.stats.mapped_bytes = 30;
        snapshot.stats.allocations = 40;
        snapshot.regions.push(crate::allocator_view::Region {
            reserved_bytes: 1_024,
            used_slices: 3,
            free_slices: 13,
            ..crate::allocator_view::Region::default()
        });
        let mut topology = crate::allocator_topology::TopologyRegion {
            slice_bytes: 64 * 1024,
            ..crate::allocator_topology::TopologyRegion::default()
        };
        for kind in [
            crate::allocator_topology::SliceKind::Small,
            crate::allocator_topology::SliceKind::Medium,
            crate::allocator_topology::SliceKind::Medium,
            crate::allocator_topology::SliceKind::Bump,
        ] {
            let mut slice = crate::allocator_topology::Slice {
                kind,
                ..crate::allocator_topology::Slice::default()
            };
            if kind == crate::allocator_topology::SliceKind::Small {
                slice.segments.push(crate::allocator_topology::Segment {
                    index: 0,
                    class_index: 2,
                    context: false,
                    live_blocks: 0,
                    usable_blocks: 100,
                    utilization_tracked: false,
                });
            }
            if kind == crate::allocator_topology::SliceKind::Medium {
                slice.span_slices = 2;
                slice.requested_bytes = 80_000;
                slice.usable_bytes = 96_000;
            }
            topology.slices.push(slice);
        }
        snapshot.topology.push(topology);
        snapshot.size_classes.push(crate::allocator_view::SizeClass {
            class_index: 2,
            block_bytes: 64,
            live_allocations: crate::allocator_view::Estimate {
                value: 25,
                lower_bound: 24,
                upper_bound: 26,
            },
            requested_bytes: crate::allocator_view::Estimate {
                value: 1_200,
                lower_bound: 1_100,
                upper_bound: 1_300,
            },
            usable_bytes: crate::allocator_view::Estimate {
                value: 1_600,
                lower_bound: 1_536,
                upper_bound: 1_664,
            },
        });

        assert_eq!(
            MemorySnapshot::from_snapshot(&snapshot),
            MemorySnapshot {
                live_bytes: 10,
                peak_live_bytes: 20,
                peak_live_bytes_scope: crate::allocator_view::PeakLiveBytesScope::Unavailable,
                mapped_bytes: 30,
                allocations: 40,
                reserved_bytes: 1_024,
                used_slices: 3,
                free_slices: 13,
                slice_bytes: 64 * 1024,
                small_slices: 1,
                medium_slices: 2,
                bump_slices: 1,
                unknown_slices: 0,
                regions: vec![MemoryRegion {
                    index: 0,
                    reserved_bytes: 1_024,
                    used_slices: 3,
                    free_slices: 13,
                }],
                size_classes: vec![MemorySizeClass {
                    block_bytes: 64,
                    live_allocations: 25,
                    capacity_blocks: 100,
                    requested_bytes: 1_200,
                    usable_bytes: 1_600,
                }],
                medium_allocations: MediumAllocations {
                    count: 2,
                    requested_bytes: 160_000,
                    usable_bytes: 192_000,
                    span_slices: 4,
                    largest_requested_bytes: 80_000,
                },
                tiers: vec![
                    MemoryTierData {
                        kind: MemoryTier::Small,
                        current_allocations: 25,
                        current_bytes: 1_200,
                        buckets: vec![MemoryBucket {
                            lower_bytes: 1,
                            upper_bytes: 64,
                            allocations: 0,
                            allocated_bytes: 0,
                            live_allocations: 0,
                            live_bytes: 0,
                            topology_live_allocations: Some(25),
                            capacity_blocks: Some(100),
                            requested_bytes: Some(1_200),
                            usable_bytes: Some(1_600),
                            hotspots: Vec::new(),
                        }],
                    },
                    MemoryTierData {
                        kind: MemoryTier::Medium,
                        current_allocations: 2,
                        current_bytes: 160_000,
                        buckets: Vec::new(),
                    },
                    MemoryTierData {
                        kind: MemoryTier::Direct,
                        current_allocations: 0,
                        current_bytes: 0,
                        buckets: Vec::new(),
                    },
                ],
            }
        );
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "the single assertion covers every retained allocation routing boundary"
    )]
    fn memory_snapshot_groups_retained_allocations_by_routing_shape() {
        let mut snapshot = crate::allocator_view::Snapshot::new(crate::allocator_view::Version::new(0, 1, 0));
        snapshot.size_classes.push(crate::allocator_view::SizeClass {
            class_index: 0,
            block_bytes: 64,
            live_allocations: crate::allocator_view::Estimate::default(),
            requested_bytes: crate::allocator_view::Estimate::default(),
            usable_bytes: crate::allocator_view::Estimate::default(),
        });
        snapshot.size_classes.push(crate::allocator_view::SizeClass {
            class_index: 1,
            block_bytes: 4_096,
            live_allocations: crate::allocator_view::Estimate::default(),
            requested_bytes: crate::allocator_view::Estimate::default(),
            usable_bytes: crate::allocator_view::Estimate::default(),
        });
        let event = |allocation_id, size, align, address| {
            Event::from_fields(EventFields {
                thread_log_id: 1,
                event_thread_id: 1,
                sequence: allocation_id,
                allocation_id,
                kind: EventKind::Allocated,
                heap_id: 1,
                heap_kind: HeapKind::General,
                freed_after_heap_release: false,
                address: allocation_id * 16,
                size,
                align,
                call_stack: vec![address],
            })
        };
        snapshot.callers = Some(Callers::from_fields(CallersFields {
            session_id: 1,
            total_events: 7,
            lost_events: 0,
            threads: Vec::new(),
            events: vec![
                event(1, 32, 8, 0x1000),
                event(2, 100_000, 8, 0x2000),
                event(3, 32, 128 * 1024, 0x3000),
                event(4, 2_000, 2_048, 0x1000),
                event(5, 64, 65_540, 0x3000),
                event(6, 3_000_000, 8, 0x2000),
                event(7, 2_048, 8_192, 0x2000),
            ],
            thread_names: Vec::new(),
        }));
        snapshot.addresses = [("app::small", 0x1000), ("app::medium", 0x2000), ("app::direct", 0x3000)]
            .into_iter()
            .map(|(symbol, address)| {
                AddressLookup::from_fields(AddressLookupFields {
                    address,
                    symbol: Some(symbol.into()),
                    filename: None,
                    line: None,
                    column: None,
                })
            })
            .collect();

        let memory = MemorySnapshot::from_snapshot(&snapshot);

        assert_eq!(
            memory
                .tiers
                .iter()
                .map(|tier| {
                    (
                        tier.kind,
                        tier.buckets
                            .iter()
                            .map(|bucket| {
                                (
                                    bucket.lower_bytes,
                                    bucket.upper_bytes,
                                    bucket.allocations,
                                    bucket.allocated_bytes,
                                    bucket.live_allocations,
                                    bucket.live_bytes,
                                    bucket.hotspots[0].location(AllocationStackFilter::Application),
                                )
                            })
                            .collect::<Vec<_>>(),
                    )
                })
                .collect::<Vec<_>>(),
            vec![
                (
                    MemoryTier::Small,
                    vec![(1, 64, 1, 32, 1, 32, "app::small"), (65, 4_096, 1, 2_000, 1, 2_000, "app::small"),],
                ),
                (
                    MemoryTier::Medium,
                    vec![
                        (2_048, 4_095, 1, 2_048, 1, 2_048, "app::medium"),
                        (65_536, 131_071, 1, 100_000, 1, 100_000, "app::medium"),
                        (2_097_152, 4_194_303, 1, 3_000_000, 1, 3_000_000, "app::medium"),
                    ],
                ),
                (
                    MemoryTier::Direct,
                    vec![(32, 63, 1, 32, 1, 32, "app::direct"), (64, 127, 1, 64, 1, 64, "app::direct"),],
                ),
            ]
        );
    }

    #[test]
    fn every_internal_frame_marker_is_filtered_independently() {
        let lookup = |symbol: Option<&str>, filename: Option<&str>| {
            AddressLookup::from_fields(AddressLookupFields {
                address: 1,
                symbol: symbol.map(str::to_owned),
                filename: filename.map(str::to_owned),
                line: None,
                column: None,
            })
        };
        let primitive_symbols = [
            "performables::operation",
            "seismograph::record",
            "std::thread::spawn",
            "alloc::vec::Vec",
            "core::option::Option",
            "backtrace::trace",
        ];
        let primitive_files = [
            "/repo/performables/src/lib.rs",
            "/repo/seismograph/src/lib.rs",
            "/rust/library/std/src/thread.rs",
            "/rust/library/alloc/src/vec.rs",
            "/rust/library/core/src/option.rs",
        ];
        let allocation_symbols = [
            "rallocator::allocate",
            "seismograph::record",
            "seismograph_rallocator::capture",
            "std::alloc::alloc",
            "alloc::alloc::alloc",
            "core::ptr::write",
        ];
        let allocation_files = [
            "/repo/rallocator/src/lib.rs",
            "/repo/seismograph/src/lib.rs",
            "/repo/seismograph_rallocator/src/lib.rs",
            "/rust/library/std/src/alloc.rs",
            "/rust/library/alloc/src/alloc.rs",
            "/rust/library/core/src/ptr.rs",
        ];

        assert_eq!(
            (
                primitive_symbols.map(|symbol| is_internal_primitive_frame(Some(&lookup(Some(symbol), None)))),
                primitive_files.map(|filename| is_internal_primitive_frame(Some(&lookup(None, Some(filename))))),
                allocation_symbols.map(|symbol| is_internal_allocation_frame(Some(&lookup(Some(symbol), None)))),
                allocation_files.map(|filename| is_internal_allocation_frame(Some(&lookup(None, Some(filename))))),
                is_internal_primitive_frame(Some(&lookup(Some("app::run"), Some("/repo/app.rs")))),
                is_internal_allocation_frame(Some(&lookup(Some("app::run"), Some("/repo/app.rs")))),
            ),
            ([true; 6], [true; 5], [true; 6], [true; 6], false, false)
        );
    }

    #[test]
    fn allocation_liveness_pairs_remote_frees_before_allocations_by_owner_and_id() {
        let mut snapshot = crate::allocator_view::Snapshot::new(crate::allocator_view::Version::new(0, 1, 0));
        let event = |owner, thread, sequence, kind, size, stack| {
            Event::from_fields(EventFields {
                thread_log_id: owner,
                event_thread_id: thread,
                sequence,
                allocation_id: 7,
                kind,
                heap_id: 1,
                heap_kind: HeapKind::General,
                freed_after_heap_release: false,
                address: 0x1000,
                size,
                align: 8,
                call_stack: vec![stack],
            })
        };
        snapshot.callers = Some(Callers::from_fields(CallersFields {
            session_id: 1,
            total_events: 3,
            lost_events: 0,
            threads: Vec::new(),
            events: vec![
                event(1, 2, 1, EventKind::Deallocated, 65_536, 0x3000),
                event(1, 1, 100, EventKind::Allocated, 65_536, 0x1000),
                event(2, 2, 100, EventKind::Allocated, 32, 0x2000),
            ],
            thread_names: Vec::new(),
        }));
        let allocations = AllocationSnapshot::from_snapshot(&snapshot);
        assert_eq!(
            allocations
                .hotspots
                .iter()
                .map(|hotspot| (hotspot.allocated_bytes, hotspot.live_allocations, hotspot.live_bytes))
                .collect::<Vec<_>>(),
            [(65_536, 0, 0), (32, 1, 32)],
        );
    }

    #[test]
    fn allocation_snapshot_ranks_hotspots_by_count_then_bytes() {
        let mut snapshot = crate::allocator_view::Snapshot::new(crate::allocator_view::Version::new(0, 1, 0));
        let event = |allocation_id, kind, size, call_stack| {
            Event::from_fields(EventFields {
                thread_log_id: 1,
                event_thread_id: 1,
                sequence: allocation_id,
                allocation_id,
                kind,
                heap_id: 1,
                heap_kind: HeapKind::General,
                freed_after_heap_release: false,
                address: allocation_id * 16,
                size,
                align: 8,
                call_stack,
            })
        };
        snapshot.callers = Some(Callers::from_fields(CallersFields {
            session_id: 7,
            total_events: 4,
            lost_events: 0,
            threads: Vec::new(),
            events: vec![
                event(1, EventKind::Allocated, 64, vec![0x3000, 0x1000]),
                event(2, EventKind::Allocated, 32, vec![0x3000, 0x1000]),
                event(1, EventKind::Deallocated, 64, Vec::new()),
                event(3, EventKind::Allocated, 4_096, vec![0x2000]),
            ],
            thread_names: Vec::new(),
        }));
        snapshot.addresses.push(AddressLookup::from_fields(AddressLookupFields {
            address: 0x1000,
            symbol: Some("app::small_allocations".into()),
            filename: Some(r"C:\src\app.rs".into()),
            line: Some(42),
            column: None,
        }));
        snapshot.addresses.push(AddressLookup::from_fields(AddressLookupFields {
            address: 0x2000,
            symbol: Some("app::large_allocation".into()),
            filename: Some(r"C:\src\large.rs".into()),
            line: Some(9),
            column: None,
        }));
        snapshot.addresses.push(AddressLookup::from_fields(AddressLookupFields {
            address: 0x3000,
            symbol: Some("rallocator::allocator::allocate".into()),
            filename: Some(r"C:\src\rallocator\src\allocator.rs".into()),
            line: Some(100),
            column: None,
        }));

        let mut actual = AllocationSnapshot::from_snapshot(&snapshot);
        assert_eq!(actual.records.len(), 4);
        actual.records.clear();
        assert_eq!(
            actual,
            AllocationSnapshot {
                records: Vec::new(),
                thread_count: 0,
                total_events: 4,
                retained_events: 4,
                lost_events: 0,
                hotspots: vec![
                    AllocationHotspot {
                        allocations: 2,
                        allocated_bytes: 96,
                        live_allocations: 1,
                        live_bytes: 32,
                        application_stack: vec!["app::small_allocations (app.rs:42)".into()],
                        complete_stack: vec![
                            "rallocator::allocator::allocate (allocator.rs:100)".into(),
                            "app::small_allocations (app.rs:42)".into(),
                        ],
                    },
                    AllocationHotspot {
                        allocations: 1,
                        allocated_bytes: 4_096,
                        live_allocations: 1,
                        live_bytes: 4_096,
                        application_stack: vec!["app::large_allocation (large.rs:9)".into()],
                        complete_stack: vec!["app::large_allocation (large.rs:9)".into()],
                    },
                ],
            }
        );
    }

    #[test]
    fn primitive_snapshot_aggregates_operations_and_application_stacks() {
        let event = |thread, sequence, object, kind, stack: Vec<u64>| RuntimeEvent {
            thread_id: ThreadId::new(thread),
            sequence: EventSequence::new(sequence),
            timestamp: EventTimestamp::from_ticks(sequence),
            kind,
            payload: EventPayload::Object(ObjectId::new(object)),
            call_stack: stack.into_iter().map(RuntimeAddress::new).collect(),
        };
        let events = vec![
            event(1, 1, 42, RuntimeEventKind::ArcDeref, vec![0x3000, 0x1000]),
            event(2, 1, 42, RuntimeEventKind::ArcDeref, vec![0x3000, 0x1000]),
            event(1, 2, 43, RuntimeEventKind::ArcClone, vec![0x2000]),
        ];
        let addresses = vec![
            AddressLookup::from_fields(AddressLookupFields {
                address: 0x1000,
                symbol: Some("app::read_shared".into()),
                filename: Some(r"C:\src\app.rs".into()),
                line: Some(10),
                column: None,
            }),
            AddressLookup::from_fields(AddressLookupFields {
                address: 0x2000,
                symbol: Some("app::clone_shared".into()),
                filename: Some(r"C:\src\app.rs".into()),
                line: Some(20),
                column: None,
            }),
            AddressLookup::from_fields(AddressLookupFields {
                address: 0x3000,
                symbol: Some("performables::arc::Arc::deref".into()),
                filename: Some(r"C:\src\performables\src\arc\mod.rs".into()),
                line: Some(708),
                column: None,
            }),
        ];

        let snapshot = PrimitiveSnapshot::from_events(3, 0, &events, &addresses);

        assert_eq!(
            snapshot.groups[0],
            PrimitiveGroup {
                kind: PrimitiveKind::Arc,
                events: 3,
                objects: 2,
                contentions: 0,
                operations: vec![
                    PrimitiveOperation {
                        kind: PrimitiveOperationKind::ArcCreate,
                        events: 0,
                        objects: 0,
                        threads: 0,
                        hotspots: Vec::new(),
                    },
                    PrimitiveOperation {
                        kind: PrimitiveOperationKind::ArcClone,
                        events: 1,
                        objects: 1,
                        threads: 1,
                        hotspots: vec![PrimitiveHotspot {
                            count: 1,
                            application_stack: vec!["app::clone_shared (app.rs:20)".into()],
                            complete_stack: vec!["app::clone_shared (app.rs:20)".into()],
                        }],
                    },
                    PrimitiveOperation {
                        kind: PrimitiveOperationKind::ArcDeref,
                        events: 2,
                        objects: 1,
                        threads: 2,
                        hotspots: vec![PrimitiveHotspot {
                            count: 2,
                            application_stack: vec!["app::read_shared (app.rs:10)".into()],
                            complete_stack: vec![
                                "performables::arc::Arc::deref (mod.rs:708)".into(),
                                "app::read_shared (app.rs:10)".into(),
                            ],
                        }],
                    },
                    PrimitiveOperation {
                        kind: PrimitiveOperationKind::ArcDrop,
                        events: 0,
                        objects: 0,
                        threads: 0,
                        hotspots: Vec::new(),
                    },
                    PrimitiveOperation {
                        kind: PrimitiveOperationKind::ArcRelocate,
                        events: 0,
                        objects: 0,
                        threads: 0,
                        hotspots: Vec::new(),
                    },
                ],
            }
        );
    }

    #[test]
    fn primitive_snapshot_counts_contentions_by_type() {
        let event = |sequence, object, kind| RuntimeEvent {
            thread_id: ThreadId::new(1),
            sequence: EventSequence::new(sequence),
            timestamp: EventTimestamp::from_ticks(sequence),
            kind,
            payload: EventPayload::Object(ObjectId::new(object)),
            call_stack: Vec::new(),
        };
        let events = vec![
            event(1, 10, RuntimeEventKind::MutexContention),
            event(2, 10, RuntimeEventKind::MutexContention),
            event(3, 20, RuntimeEventKind::RwLockReadContention),
            event(4, 20, RuntimeEventKind::RwLockWriteContention),
            event(5, 20, RuntimeEventKind::RwLockReadAccess),
        ];

        let snapshot = PrimitiveSnapshot::from_events(5, 0, &events, &[]);

        assert_eq!(
            snapshot.groups.iter().map(|group| group.contentions).collect::<Vec<_>>(),
            vec![0, 2, 2, 0, 0, 0, 0]
        );
    }

    #[test]
    fn channel_receive_waits_remain_visible_without_counting_as_contention() {
        let events = [RuntimeEventKind::ChannelReceiveContention, RuntimeEventKind::ChannelSendContention]
            .into_iter()
            .map(|kind| RuntimeEvent {
                thread_id: ThreadId::new(1),
                sequence: EventSequence::new(1),
                timestamp: EventTimestamp::from_ticks(1),
                kind,
                payload: EventPayload::Object(ObjectId::new(7)),
                call_stack: Vec::new(),
            })
            .collect::<Vec<_>>();
        let snapshot = PrimitiveSnapshot::from_events(2, 0, &events, &[]);
        let channel = snapshot.groups.iter().find(|group| group.kind == PrimitiveKind::Channel).unwrap();
        let receive = channel
            .operations
            .iter()
            .find(|operation| operation.kind == PrimitiveOperationKind::ChannelReceiveContention)
            .unwrap();
        assert_eq!(
            (channel.events, channel.contentions, receive.events, receive.kind.is_contention()),
            (2, 1, 1, false)
        );
        assert!(!ThreadOperationKind::ChannelReceiveContention.is_contention());
        assert!(ThreadOperationKind::ChannelSendContention.is_contention());
    }

    #[test]
    fn thread_snapshot_links_same_thread_allocations_and_channels() {
        let events = [
            (10, RuntimeEventKind::Allocation),
            (10, RuntimeEventKind::Deallocation),
            (20, RuntimeEventKind::ChannelSend),
            (20, RuntimeEventKind::ChannelReceive),
        ]
        .into_iter()
        .enumerate()
        .map(|(index, (object, kind))| RuntimeEvent {
            thread_id: ThreadId::new(1),
            sequence: EventSequence::new(u64::try_from(index).unwrap()),
            timestamp: EventTimestamp::from_ticks(1),
            kind,
            payload: EventPayload::Object(ObjectId::new(object)),
            call_stack: vec![RuntimeAddress::new(0x1000)],
        })
        .collect();
        let snapshot = ThreadSnapshot::from_events(
            &Events {
                events,
                ..Events::default()
            },
            &[],
        );
        let interactions = snapshot.threads[0]
            .operations
            .iter()
            .filter(|operation| operation.events > 0)
            .map(|operation| {
                let participant = &operation.participants[0];
                let object = &participant.objects[0];
                (
                    operation.kind,
                    participant.thread_id,
                    object.object_id,
                    object.selected_events,
                    object.related_events,
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            interactions,
            [
                (ThreadOperationKind::Allocation, 1, 10, 1, 1),
                (ThreadOperationKind::Deallocation, 1, 10, 1, 1),
                (ThreadOperationKind::ChannelSend, 1, 20, 1, 2),
                (ThreadOperationKind::ChannelReceive, 1, 20, 1, 2),
            ]
        );
    }

    #[test]
    fn lock_poison_events_correlate_with_their_lock_object_group() {
        let event = |sequence, object, kind| RuntimeEvent {
            thread_id: ThreadId::new(1),
            sequence: EventSequence::new(sequence),
            timestamp: EventTimestamp::from_ticks(sequence),
            kind,
            payload: EventPayload::Object(ObjectId::new(object)),
            call_stack: Vec::new(),
        };
        let events = vec![
            event(1, 10, RuntimeEventKind::MutexAccess),
            event(2, 10, RuntimeEventKind::LockPoisoned),
            event(3, 20, RuntimeEventKind::RwLockWriteAccess),
            event(4, 20, RuntimeEventKind::LockPoisonObserved),
        ];

        let snapshot = PrimitiveSnapshot::from_events(4, 0, &events, &[]);
        let mutex = snapshot.groups.iter().find(|group| group.kind == PrimitiveKind::Mutex).unwrap();
        let rw_lock = snapshot.groups.iter().find(|group| group.kind == PrimitiveKind::RwLock).unwrap();
        let mutex_poisoned = mutex
            .operations
            .iter()
            .find(|operation| operation.kind == PrimitiveOperationKind::LockPoisoned)
            .unwrap();
        let mutex_observed = mutex
            .operations
            .iter()
            .find(|operation| operation.kind == PrimitiveOperationKind::LockPoisonObserved)
            .unwrap();
        let rw_poisoned = rw_lock
            .operations
            .iter()
            .find(|operation| operation.kind == PrimitiveOperationKind::LockPoisoned)
            .unwrap();
        let rw_observed = rw_lock
            .operations
            .iter()
            .find(|operation| operation.kind == PrimitiveOperationKind::LockPoisonObserved)
            .unwrap();

        assert_eq!(
            (
                mutex.objects,
                mutex_poisoned.events,
                mutex_observed.events,
                rw_lock.objects,
                rw_poisoned.events,
                rw_observed.events,
            ),
            (1, 1, 0, 1, 0, 1)
        );
    }

    #[test]
    fn runtime_snapshot_keeps_events_without_allocator_addresses() {
        let decoded = seismograph::snapshot::DecodedSnapshot {
            capture_duration_nanos: 0,
            events: Events {
                clock: EventClock::ProcessMonotonic,
                total_events: 1,
                lost_events: 0,
                recording: RecordingPolicies::default(),
                threads: vec![ThreadLog {
                    thread_id: ThreadId::new(1),
                    total_events: 1,
                    lost_events: 0,
                    name: "worker".into(),
                }],
                events: vec![RuntimeEvent {
                    thread_id: ThreadId::new(1),
                    sequence: EventSequence::new(1),
                    timestamp: EventTimestamp::from_ticks(1),
                    kind: RuntimeEventKind::ArcDeref,
                    payload: EventPayload::Object(ObjectId::new(7)),
                    call_stack: vec![RuntimeAddress::new(0x1000)],
                }],
            },
            sources: Vec::new(),
        };

        let runtime = RuntimeSnapshot::from_events(&decoded, &[], None);

        assert_eq!(
            (
                runtime
                    .primitives
                    .groups
                    .iter()
                    .flat_map(|group| &group.operations)
                    .map(|operation| operation.events)
                    .sum::<u64>(),
                runtime.threads.threads.len(),
            ),
            (1, 1)
        );
    }

    #[test]
    fn io_and_cache_snapshots_aggregate_resources_operations_and_outcomes() {
        let event = |sequence, timestamp, kind, payload| RuntimeEvent {
            thread_id: ThreadId::new(7),
            sequence: EventSequence::new(sequence),
            timestamp: EventTimestamp::from_ticks(timestamp),
            kind,
            payload,
            call_stack: Vec::new(),
        };
        let io = |operation, resource, requested, completed, outcome| {
            EventPayload::Io(IoEvent {
                operation_id: IoOperationId::from_raw(operation).unwrap(),
                resource_id: IoResourceId::from_raw(resource).unwrap(),
                buffer_id: None,
                requested_bytes: requested,
                completed_bytes: completed,
                buffer_len: requested,
                buffer_span_count: 1,
                resource_kind: if resource == 1 {
                    IoResourceKind::File
                } else {
                    IoResourceKind::TcpStream
                },
                outcome,
            })
        };
        let cache = |tier, fallback| {
            EventPayload::Numeric(NumericEvent {
                object_id: ObjectId::new(tier),
                value: u64::from(fallback),
            })
        };
        let events = Events {
            clock: EventClock::ProcessMonotonic,
            total_events: 10,
            lost_events: 1,
            recording: RecordingPolicies::default(),
            threads: Vec::new(),
            events: vec![
                event(1, 10, RuntimeEventKind::IoReadStarted, io(1, 1, 100, 0, IoOutcome::Pending)),
                event(2, 30, RuntimeEventKind::IoReadFinished, io(1, 1, 100, 80, IoOutcome::Success)),
                event(3, 20, RuntimeEventKind::IoWriteStarted, io(2, 2, 50, 0, IoOutcome::Pending)),
                event(4, 40, RuntimeEventKind::IoWriteFinished, io(2, 2, 50, 0, IoOutcome::Error)),
                event(5, 50, RuntimeEventKind::CacheHit, cache(10, false)),
                event(6, 60, RuntimeEventKind::CacheMiss, cache(10, false)),
                event(7, 70, RuntimeEventKind::CacheGetError, cache(10, false)),
                event(8, 80, RuntimeEventKind::CacheRefreshHit, cache(20, true)),
                event(9, 90, RuntimeEventKind::MutexAccess, io(3, 1, 1, 0, IoOutcome::Pending)),
                event(
                    10,
                    100,
                    RuntimeEventKind::CacheHit,
                    EventPayload::Runtime(RuntimeEventPayload {
                        runtime_id: RuntimeId::from_raw(1).unwrap(),
                        worker_id: None,
                        subject_id: 0,
                        related_id: 0,
                        value_0: 0,
                        value_1: 0,
                    }),
                ),
            ],
        };

        let io = IoMonitorSnapshot::from_events(&events);
        let cache = CacheMonitorSnapshot::from_events(&events);

        assert_eq!(
            (
                io.total_events,
                io.retained_events,
                io.lost_events,
                io.resources
                    .iter()
                    .map(|resource| (
                        resource.resource_id,
                        resource.reads,
                        resource.writes,
                        resource.completed_bytes,
                        resource.errors,
                        resource.operations[0].duration_nanos,
                    ))
                    .collect::<Vec<_>>(),
                cache.total_events,
                cache.retained_events,
                cache.lost_events,
                cache
                    .tiers
                    .iter()
                    .map(|tier| (tier.tier_id, tier.fallback, tier.events, tier.hits, tier.misses, tier.errors))
                    .collect::<Vec<_>>(),
            ),
            (
                10,
                5,
                1,
                vec![(1, 1, 0, 80, 0, Some(20)), (2, 0, 1, 0, 1, Some(20))],
                10,
                4,
                1,
                vec![(10, false, 3, 1, 1, 1), (20, true, 1, 1, 0, 0)],
            )
        );
    }

    #[test]
    fn runtime_monitor_retains_only_coherent_ready_samples_across_workers() {
        let events = [
            (10, 1, 1_000, 0),
            (10, 1, 100, 1),
            (10, 2, 0, 2),
            (10, 2, 2, 2),
            (11, 1, 0, 0),
            (12, 1, 0, 2),
        ]
        .into_iter()
        .enumerate()
        .map(|(index, (task_id, worker_id, nanos, wake_flag))| RuntimeEvent {
            thread_id: ThreadId::new(worker_id),
            sequence: EventSequence::new(u64::try_from(index).unwrap()),
            timestamp: EventTimestamp::from_ticks(u64::try_from(index).unwrap()),
            kind: RuntimeEventKind::TaskPollStarted,
            payload: EventPayload::Runtime(RuntimeEventPayload {
                runtime_id: RuntimeId::from_raw(1).unwrap(),
                worker_id: Some(WorkerId::from_raw(worker_id).unwrap()),
                subject_id: task_id,
                related_id: 0,
                value_0: nanos,
                value_1: wake_flag,
            }),
            call_stack: Vec::new(),
        })
        .collect();
        let snapshot = RuntimeMonitorSnapshot::from_events(
            &Events {
                clock: EventClock::Unspecified,
                total_events: 6,
                lost_events: 0,
                recording: RecordingPolicies::default(),
                threads: Vec::new(),
                events,
            },
            None,
            &[],
        );

        assert_eq!(
            snapshot
                .workers
                .iter()
                .map(|worker| {
                    (
                        worker.worker_id,
                        worker
                            .tasks
                            .iter()
                            .map(|task| {
                                (
                                    task.task_id,
                                    task.metrics.ready_samples.to_vec(),
                                    task.metrics.poll_count,
                                    task.state.as_str(),
                                )
                            })
                            .collect::<Vec<_>>(),
                    )
                })
                .collect::<Vec<_>>(),
            vec![
                (
                    Some(1),
                    vec![
                        (10, vec![0, 2], 0, "Unknown"),
                        (11, vec![], 0, "Unknown"),
                        (12, vec![0], 0, "Unknown")
                    ]
                ),
                (Some(2), vec![(10, vec![0, 2], 0, "Unknown")]),
            ]
        );
    }

    #[test]
    fn runtime_monitor_poll_statistics_use_only_retained_finished_polls() {
        let events = [
            (10, 1, RuntimeEventKind::TaskPollStarted),
            (10, 2, RuntimeEventKind::TaskPollStarted),
            (10, 10, RuntimeEventKind::TaskPollFinished),
            (10, 11, RuntimeEventKind::TaskPollStarted),
            (10, 11, RuntimeEventKind::TaskPollFinished),
            (10, 13, RuntimeEventKind::TaskPollStarted),
            (10, 13, RuntimeEventKind::TaskPollFinished),
            (10, 113, RuntimeEventKind::TaskPollStarted),
            (10, 114, RuntimeEventKind::TaskPollStarted),
            (11, 120, RuntimeEventKind::TaskPollStarted),
            (12, 200, RuntimeEventKind::TaskPollFinished),
            (12, 200, RuntimeEventKind::TaskPollStarted),
            (13, 0, RuntimeEventKind::TaskPollFinished),
            (13, u64::MAX, RuntimeEventKind::TaskPollStarted),
        ]
        .into_iter()
        .enumerate()
        .map(|(index, (task_id, timestamp, kind))| RuntimeEvent {
            thread_id: ThreadId::new(1),
            sequence: EventSequence::new(u64::try_from(index).unwrap()),
            timestamp: EventTimestamp::from_ticks(timestamp),
            kind,
            payload: EventPayload::Runtime(RuntimeEventPayload {
                runtime_id: RuntimeId::from_raw(1).unwrap(),
                worker_id: Some(WorkerId::from_raw(1).unwrap()),
                subject_id: task_id,
                related_id: 0,
                value_0: if kind == RuntimeEventKind::TaskPollFinished && task_id == 10 {
                    timestamp - 8
                } else {
                    0
                },
                value_1: 0,
            }),
            call_stack: Vec::new(),
        })
        .collect();
        let snapshot = RuntimeMonitorSnapshot::from_events(
            &Events {
                clock: EventClock::Unspecified,
                total_events: 14,
                lost_events: 0,
                recording: RecordingPolicies::default(),
                threads: Vec::new(),
                events,
            },
            None,
            &[],
        );

        assert_eq!(
            snapshot.workers[0]
                .tasks
                .iter()
                .map(|task| (
                    task.task_id,
                    task.metrics.median_poll_nanos,
                    task.metrics.poll_count,
                    task.metrics.max_poll_nanos,
                ))
                .collect::<Vec<_>>(),
            vec![
                (10, Some(3), 3, Some(5)),
                (11, None, 0, None),
                (12, Some(0), 1, Some(0)),
                (13, Some(0), 1, Some(0)),
            ]
        );
    }

    #[test]
    fn runtime_monitor_summarizes_worker_and_task_retained_execution() {
        let event = |sequence, timestamp, kind, subject_id, value_0, value_1| RuntimeEvent {
            thread_id: ThreadId::new(7),
            sequence: EventSequence::new(sequence),
            timestamp: EventTimestamp::from_ticks(timestamp),
            kind,
            payload: EventPayload::Runtime(RuntimeEventPayload {
                runtime_id: RuntimeId::from_raw(1).unwrap(),
                worker_id: (kind != RuntimeEventKind::TaskSpawned).then(|| WorkerId::from_raw(2).unwrap()),
                subject_id,
                related_id: 0,
                value_0,
                value_1,
            }),
            call_stack: Vec::new(),
        };
        let events = vec![
            event(1, 50, RuntimeEventKind::TaskSpawned, 10, 42, 0),
            event(2, 100, RuntimeEventKind::TaskPollStarted, 10, 30, 2),
            event(3, 300, RuntimeEventKind::TaskPollFinished, 10, 200, 0),
            event(4, 500, RuntimeEventKind::TaskPollStarted, 10, 80, 2),
            event(5, 900, RuntimeEventKind::TaskPollFinished, 10, 400, 0),
        ];

        let snapshot = RuntimeMonitorSnapshot::from_events(
            &Events {
                clock: EventClock::Unspecified,
                total_events: 5,
                lost_events: 0,
                recording: RecordingPolicies::default(),
                threads: Vec::new(),
                events,
            },
            None,
            &[],
        );
        let worker = &snapshot.workers[0];
        let task = &worker.tasks[0];

        assert_eq!(
            (
                snapshot.total_events,
                snapshot.retained_events,
                snapshot.lost_events,
                task.state.as_str(),
            ),
            (5, 5, 0, "Unknown")
        );
        assert_eq!(
            (
                worker.worker_id,
                worker.metrics.poll_count,
                worker.metrics.median_poll_nanos,
                worker.metrics.max_poll_nanos,
                worker.observed_tasks,
                task.task_id,
                task.type_descriptor_id,
                task.metrics.poll_count,
                task.metrics.median_poll_nanos,
                task.metrics.max_poll_nanos,
            ),
            (Some(2), 2, Some(300), Some(400), 1, 10, Some(42), 2, Some(300), Some(400))
        );
        assert_eq!(
            (
                task.metrics.polls.as_slice(),
                task.metrics.ready_samples.as_ref(),
                task.activity.running_for,
                task.activity.ready_for,
            ),
            (
                &[Interval { start: 100, end: 300 }, Interval { start: 500, end: 900 }][..],
                &[30, 80][..],
                None,
                None
            )
        );
        assert!((worker.metrics.executing_fraction.unwrap() - 600.0 / 850.0).abs() < f64::EPSILON);

        let mut worker = worker.clone();
        let mut other = task.clone();
        other.task_id = 11;
        other.metrics.max_poll_nanos = Some(20);
        worker.tasks.push(other);
        assert_eq!(
            worker
                .sorted_tasks(RuntimeTaskSort::MaximumPoll, true)
                .into_iter()
                .map(|task| task.task_id)
                .collect::<Vec<_>>(),
            vec![10, 11]
        );
    }

    #[test]
    fn runtime_poll_metrics_stay_with_the_executing_worker_after_migration() {
        let events = [
            (1, 1, 100, RuntimeEventKind::TaskPollFinished, 20),
            (1, 2, 200, RuntimeEventKind::TaskPollFinished, 80),
            (1, 99, 210, RuntimeEventKind::TaskReady, 0),
            (2, 1, 220, RuntimeEventKind::TaskPollFinished, 10),
        ]
        .into_iter()
        .enumerate()
        .map(|(sequence, (runtime_id, worker_id, timestamp, kind, duration))| RuntimeEvent {
            thread_id: ThreadId::new(worker_id),
            sequence: EventSequence::new(u64::try_from(sequence).unwrap()),
            timestamp: EventTimestamp::from_ticks(timestamp),
            kind,
            payload: EventPayload::Runtime(RuntimeEventPayload {
                runtime_id: RuntimeId::from_raw(runtime_id).unwrap(),
                worker_id: Some(WorkerId::from_raw(worker_id).unwrap()),
                subject_id: 10,
                related_id: 0,
                value_0: duration,
                value_1: 0,
            }),
            call_stack: Vec::new(),
        })
        .collect();
        let snapshot = RuntimeMonitorSnapshot::from_events(
            &Events {
                events,
                ..Events::default()
            },
            None,
            &[],
        );
        assert_eq!(
            snapshot
                .workers
                .iter()
                .map(|worker| {
                    let task = &worker.tasks[0];
                    (
                        worker.runtime_id,
                        worker.worker_id,
                        worker.observed_tasks,
                        worker.metrics.poll_count,
                        task.metrics.poll_count,
                        task.metrics.median_poll_nanos,
                        task.metrics.max_poll_nanos,
                        task.worker_ids.clone(),
                    )
                })
                .collect::<Vec<_>>(),
            [
                (1, Some(1), 1, 1, 1, Some(20), Some(20), vec![1, 2]),
                (1, Some(2), 1, 1, 1, Some(80), Some(80), vec![1, 2]),
                (2, Some(1), 1, 1, 1, Some(10), Some(10), vec![1]),
            ]
        );
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "the lifetime-source fixture keeps worker, task, and retained-event metrics together"
    )]
    fn runtime_source_lifetime_metrics_do_not_override_retained_event_counts() {
        use seismograph_runtime::snapshot::{
            Counters, Runtime, RuntimeState, Snapshot as RuntimeSourceSnapshot, Task, TaskMetrics, Worker, WorkerState,
        };
        use seismograph_runtime::worker::WorkerRole;

        let events = Events {
            clock: EventClock::Unspecified,
            total_events: 1_000,
            lost_events: 998,
            recording: RecordingPolicies::default(),
            threads: Vec::new(),
            events: vec![
                RuntimeEvent {
                    thread_id: ThreadId::new(7),
                    sequence: EventSequence::new(1),
                    timestamp: EventTimestamp::from_ticks(100),
                    kind: RuntimeEventKind::TaskPollStarted,
                    payload: EventPayload::Runtime(RuntimeEventPayload {
                        runtime_id: RuntimeId::from_raw(1).unwrap(),
                        worker_id: Some(WorkerId::from_raw(2).unwrap()),
                        subject_id: 10,
                        related_id: 0,
                        value_0: 99,
                        value_1: 1,
                    }),
                    call_stack: Vec::new(),
                },
                RuntimeEvent {
                    thread_id: ThreadId::new(7),
                    sequence: EventSequence::new(2),
                    timestamp: EventTimestamp::from_ticks(200),
                    kind: RuntimeEventKind::TaskPollFinished,
                    payload: EventPayload::Runtime(RuntimeEventPayload {
                        runtime_id: RuntimeId::from_raw(1).unwrap(),
                        worker_id: Some(WorkerId::from_raw(2).unwrap()),
                        subject_id: 10,
                        related_id: 0,
                        value_0: 100,
                        value_1: 0,
                    }),
                    call_stack: Vec::new(),
                },
                RuntimeEvent {
                    thread_id: ThreadId::new(7),
                    sequence: EventSequence::new(3),
                    timestamp: EventTimestamp::from_ticks(300),
                    kind: RuntimeEventKind::TaskCompleted,
                    payload: EventPayload::Runtime(RuntimeEventPayload {
                        runtime_id: RuntimeId::from_raw(1).unwrap(),
                        worker_id: Some(WorkerId::from_raw(2).unwrap()),
                        subject_id: 10,
                        related_id: 0,
                        value_0: 0,
                        value_1: 0,
                    }),
                    call_stack: Vec::new(),
                },
                RuntimeEvent {
                    thread_id: ThreadId::new(7),
                    sequence: EventSequence::new(4),
                    timestamp: EventTimestamp::from_ticks(400),
                    kind: RuntimeEventKind::TaskCanceled,
                    payload: EventPayload::Runtime(RuntimeEventPayload {
                        runtime_id: RuntimeId::from_raw(1).unwrap(),
                        worker_id: Some(WorkerId::from_raw(2).unwrap()),
                        subject_id: 10,
                        related_id: 0,
                        value_0: 0,
                        value_1: 0,
                    }),
                    call_stack: Vec::new(),
                },
                RuntimeEvent {
                    thread_id: ThreadId::new(7),
                    sequence: EventSequence::new(5),
                    timestamp: EventTimestamp::from_ticks(500),
                    kind: RuntimeEventKind::TaskPanicked,
                    payload: EventPayload::Runtime(RuntimeEventPayload {
                        runtime_id: RuntimeId::from_raw(1).unwrap(),
                        worker_id: Some(WorkerId::from_raw(2).unwrap()),
                        subject_id: 10,
                        related_id: 0,
                        value_0: 0,
                        value_1: 0,
                    }),
                    call_stack: Vec::new(),
                },
                RuntimeEvent {
                    thread_id: ThreadId::new(7),
                    sequence: EventSequence::new(6),
                    timestamp: EventTimestamp::from_ticks(600),
                    kind: RuntimeEventKind::TaskMaterialized,
                    payload: EventPayload::Runtime(RuntimeEventPayload {
                        runtime_id: RuntimeId::from_raw(1).unwrap(),
                        worker_id: Some(WorkerId::from_raw(2).unwrap()),
                        subject_id: 10,
                        related_id: 0,
                        value_0: 0,
                        value_1: 0,
                    }),
                    call_stack: Vec::new(),
                },
                RuntimeEvent {
                    thread_id: ThreadId::new(7),
                    sequence: EventSequence::new(7),
                    timestamp: EventTimestamp::from_ticks(700),
                    kind: RuntimeEventKind::TaskPollStarted,
                    payload: EventPayload::Runtime(RuntimeEventPayload {
                        runtime_id: RuntimeId::from_raw(1).unwrap(),
                        worker_id: Some(WorkerId::from_raw(2).unwrap()),
                        subject_id: 10,
                        related_id: 0,
                        value_0: 0,
                        value_1: 0,
                    }),
                    call_stack: Vec::new(),
                },
            ],
        };
        let source = RuntimeSourceSnapshot {
            runtimes: vec![Runtime {
                id: RuntimeId::from_raw(1).unwrap(),
                name: "runtime".into(),
                configured_workers: 1,
                lifecycle_backtraces: seismograph::recorder::event::BacktraceCapture::Never,
                state: RuntimeState::Running,
                created_at: EventTimestamp::from_ticks(1),
                retired_at: None,
                counters: Counters::default(),
                workers: vec![
                    Worker {
                        id: WorkerId::from_raw(2).unwrap(),
                        role: WorkerRole::Core,
                        state: WorkerState::Running,
                        processor_index: None,
                        thread_id: Some(ThreadId::new(7)),
                        current_task: Some(seismograph::recorder::runtime::TaskId::from_raw(12).unwrap()),
                    },
                    Worker {
                        id: WorkerId::from_raw(3).unwrap(),
                        role: WorkerRole::Blocking,
                        state: WorkerState::Parked,
                        processor_index: None,
                        thread_id: None,
                        current_task: None,
                    },
                ],
                tasks: vec![
                    Task {
                        id: seismograph::recorder::runtime::TaskId::from_raw(10).unwrap(),
                        parent: None,
                        type_descriptor: seismograph::recorder::runtime::TypeDescriptorId::from_raw(42).unwrap(),
                        future_size_bytes: None,
                        spawned_at: EventTimestamp::from_ticks(5),
                        last_worker_id: Some(WorkerId::from_raw(2).unwrap()),
                        activity: None,
                        metrics: TaskMetrics {
                            poll_count: 500,
                            poll_duration_nanos: 10_000,
                            max_poll_duration_nanos: 300,
                            resume_count: 499,
                            resume_duration_nanos: 20_000,
                            max_resume_duration_nanos: 400,
                            ready_wait_count: 450,
                            ready_wait_duration_nanos: 9_000,
                            max_ready_wait_duration_nanos: 200,
                        },
                        spawn_backtrace: Vec::new(),
                    },
                    Task {
                        id: seismograph::recorder::runtime::TaskId::from_raw(11).unwrap(),
                        parent: None,
                        type_descriptor: seismograph::recorder::runtime::TypeDescriptorId::from_raw(43).unwrap(),
                        future_size_bytes: None,
                        spawned_at: EventTimestamp::from_ticks(0),
                        last_worker_id: None,
                        activity: None,
                        metrics: TaskMetrics::default(),
                        spawn_backtrace: Vec::new(),
                    },
                ],
            }],
            addresses: Vec::new(),
        };

        let snapshot = RuntimeMonitorSnapshot::from_events(&events, Some(&source), &[]);
        assert_eq!(
            (
                snapshot.retained_events,
                snapshot.lost_events,
                snapshot
                    .workers
                    .iter()
                    .map(|worker| {
                        (
                            worker.runtime_id,
                            worker.runtime_name.as_str(),
                            worker.worker_id,
                            worker.role.as_str(),
                            worker.state.as_str(),
                            worker.thread_id,
                            worker.current_task,
                            worker.tasks.iter().map(|task| task.task_id).collect::<Vec<_>>(),
                        )
                    })
                    .collect::<Vec<_>>(),
            ),
            (
                7,
                998,
                vec![
                    (1, "runtime", None, "Unbound", "Unknown", None, None, vec![11]),
                    (1, "runtime", Some(2), "Core", "Running", Some(7), Some(12), vec![10, 12]),
                    (1, "runtime", Some(3), "Blocking", "Parked", None, None, Vec::new()),
                ],
            )
        );
        let task = &snapshot.workers[1].tasks[0];
        assert_eq!(
            (
                task.task_id,
                task.state.as_str(),
                task.metrics.poll_count,
                task.metrics.median_poll_nanos,
                task.metrics.max_poll_nanos
            ),
            (10, "Panicked", 1, Some(100), Some(100))
        );
        let source_only = RuntimeMonitorSnapshot::from_events(&Events::default(), Some(&source), &[]);
        assert_eq!(
            source_only
                .workers
                .iter()
                .flat_map(|worker| &worker.tasks)
                .map(|task| (
                    task.task_id,
                    task.state.as_str(),
                    task.metrics.poll_count,
                    task.metrics.median_poll_nanos,
                    task.metrics.max_poll_nanos,
                    task.activity.running_for,
                    task.activity.ready_for,
                ))
                .collect::<Vec<_>>(),
            [
                (11, "Unknown", 0, None, None, None, None),
                (10, "Unknown", 0, None, None, None, None),
                (12, "Unknown", 0, None, None, None, None)
            ]
        );
    }

    #[test]
    fn runtime_source_requires_coherent_activity_before_reporting_current_ages() {
        use seismograph::recorder::runtime::{TaskId, TypeDescriptorId};
        use seismograph_runtime::snapshot::{
            Counters, Runtime, RuntimeState, Snapshot, Task, TaskActivity, TaskActivityState, TaskMetrics,
        };

        let activity = TaskActivity {
            observed_at: EventTimestamp::from_ticks(100),
            state: TaskActivityState::Running,
            ready_since: Some(EventTimestamp::from_ticks(80)),
            poll_started_at: Some(EventTimestamp::from_ticks(60)),
            poll_worker_id: None,
            queued_since: None,
        };
        let mut source = Snapshot {
            runtimes: vec![Runtime {
                id: RuntimeId::from_raw(1).unwrap(),
                name: "executor".into(),
                configured_workers: 1,
                lifecycle_backtraces: seismograph::recorder::event::BacktraceCapture::Never,
                state: RuntimeState::Running,
                created_at: EventTimestamp::from_ticks(1),
                retired_at: None,
                counters: Counters::default(),
                workers: Vec::new(),
                tasks: vec![Task {
                    id: TaskId::from_raw(1).unwrap(),
                    parent: None,
                    type_descriptor: TypeDescriptorId::from_raw(1).unwrap(),
                    future_size_bytes: None,
                    spawned_at: EventTimestamp::from_ticks(1),
                    last_worker_id: None,
                    metrics: TaskMetrics::default(),
                    activity: None,
                    spawn_backtrace: Vec::new(),
                }],
            }],
            addresses: Vec::new(),
        };
        for (activity, expected) in [
            (None, ("Unknown", None, None, false)),
            (
                Some(TaskActivity {
                    state: TaskActivityState::Unknown,
                    ..activity
                }),
                ("Unknown", None, None, false),
            ),
            (
                Some(TaskActivity {
                    poll_started_at: Some(EventTimestamp::from_ticks(101)),
                    ..activity
                }),
                ("Unknown", None, None, false),
            ),
            (Some(activity), ("Running", Some(40), None, true)),
            (
                Some(TaskActivity {
                    state: TaskActivityState::Ready,
                    poll_started_at: None,
                    queued_since: Some(EventTimestamp::from_ticks(90)),
                    ..activity
                }),
                ("Ready", None, Some(10), false),
            ),
        ] {
            source.runtimes[0].tasks[0].activity = activity;
            let snapshot = RuntimeMonitorSnapshot::from_events(&Events::default(), Some(&source), &[]);
            let task = &snapshot.workers[0].tasks[0];
            assert_eq!(
                (
                    task.activity.state.as_str(),
                    task.activity.running_for,
                    task.activity.ready_for,
                    task.activity.repoll_requested
                ),
                expected
            );
            assert_eq!(
                (task.metrics.poll_count, task.metrics.median_poll_nanos, task.metrics.max_poll_nanos),
                (0, None, None)
            );
        }
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "the single assertion verifies the complete cross-thread activity fixture"
    )]
    fn thread_snapshot_links_cross_thread_object_activity() {
        let event = |thread, sequence, object, kind, address| RuntimeEvent {
            thread_id: ThreadId::new(thread),
            sequence: EventSequence::new(sequence),
            timestamp: EventTimestamp::from_ticks(sequence),
            kind,
            payload: EventPayload::Object(ObjectId::new(object)),
            call_stack: vec![RuntimeAddress::new(address)],
        };
        let decoded = seismograph::snapshot::DecodedSnapshot {
            capture_duration_nanos: 0,
            events: Events {
                clock: EventClock::ProcessMonotonic,
                total_events: 8,
                lost_events: 0,
                recording: RecordingPolicies::default(),
                threads: vec![
                    ThreadLog {
                        thread_id: ThreadId::new(1),
                        total_events: 2,
                        lost_events: 0,
                        name: "producer".into(),
                    },
                    ThreadLog {
                        thread_id: ThreadId::new(2),
                        total_events: 2,
                        lost_events: 0,
                        name: "consumer".into(),
                    },
                    ThreadLog {
                        thread_id: ThreadId::new(3),
                        total_events: 2,
                        lost_events: 0,
                        name: "waiter".into(),
                    },
                ],
                events: vec![
                    event(1, 1, 100, RuntimeEventKind::Allocation, 0x1000),
                    event(2, 1, 100, RuntimeEventKind::Deallocation, 0x2000),
                    event(3, 1, 200, RuntimeEventKind::MutexContention, 0x3000),
                    event(2, 2, 200, RuntimeEventKind::MutexAccess, 0x4000),
                    event(1, 2, 300, RuntimeEventKind::ArcClone, 0x5000),
                    event(3, 2, 300, RuntimeEventKind::ArcDeref, 0x6000),
                    event(1, 3, 101, RuntimeEventKind::Allocation, 0x1000),
                    event(2, 3, 101, RuntimeEventKind::Deallocation, 0x2000),
                ],
            },
            sources: Vec::new(),
        };
        let addresses = vec![
            AddressLookup::from_fields(AddressLookupFields {
                address: 0x1000,
                symbol: Some("app::allocate".into()),
                filename: Some("producer.rs".into()),
                line: Some(10),
                column: None,
            }),
            AddressLookup::from_fields(AddressLookupFields {
                address: 0x2000,
                symbol: Some("app::free".into()),
                filename: Some("consumer.rs".into()),
                line: Some(20),
                column: None,
            }),
        ];

        let snapshot = ThreadSnapshot::from_events(&decoded.events, &addresses);
        let operation = |thread: usize, kind| {
            snapshot.threads[thread]
                .operations
                .iter()
                .find(|operation| operation.kind == kind)
                .unwrap()
                .participants
                .iter()
                .find(|participant| participant.thread_id != snapshot.threads[thread].thread_id)
                .unwrap()
        };
        let allocation = operation(0, ThreadOperationKind::Allocation);
        let contention = operation(2, ThreadOperationKind::MutexContention);
        let arc = operation(0, ThreadOperationKind::ArcClone);
        let allocated_object = allocation.objects.iter().find(|object| object.object_id == 100).unwrap();

        assert_eq!(
            (
                snapshot.threads.iter().map(|thread| thread.thread_id).collect::<Vec<_>>(),
                (
                    allocation.thread_id,
                    allocated_object.object_id,
                    allocated_object.selected_events,
                    allocated_object.related_events,
                ),
                allocated_object
                    .selected_stack()
                    .unwrap()
                    .stack(AllocationStackFilter::Application)
                    .to_vec(),
                allocated_object
                    .related_stack()
                    .unwrap()
                    .stack(AllocationStackFilter::Application)
                    .to_vec(),
                allocated_object.selected_stack().unwrap().count,
                (contention.thread_id, contention.objects[0].object_id),
                (arc.thread_id, arc.objects[0].object_id),
            ),
            (
                vec![1, 2, 3],
                (2, 100, 1, 1),
                vec!["app::allocate (producer.rs:10)".to_owned()],
                vec!["app::free (consumer.rs:20)".to_owned()],
                1,
                (2, 200),
                (3, 300),
            )
        );
    }
}
