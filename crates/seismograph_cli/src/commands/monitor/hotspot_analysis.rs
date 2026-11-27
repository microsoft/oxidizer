// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Reusable, evidence-backed performance findings for one retained snapshot.

use std::cmp::Ordering;

use super::data::{
    AllocationSnapshot, AllocationStackFilter, CacheMonitorSnapshot, IoMonitorSnapshot, PrimitiveSnapshot, RuntimeMonitorSnapshot,
};

const LARGE_ALLOCATION_BYTES: u64 = 256 * 1024;
const VERY_LARGE_ALLOCATION_BYTES: u64 = 1024 * 1024;
const HIGH_ALLOCATION_COUNT: u64 = 1_000;
const HIGH_ALLOCATED_BYTES: u64 = 8 * 1024 * 1024;
const CONTENTION_EVENTS: u64 = 50;
const LONG_ASYNC_POLL_NANOS: u64 = 10_000_000;
const VERY_LONG_ASYNC_POLL_NANOS: u64 = 50_000_000;
const LONG_BLOCKING_QUEUE_NANOS: u64 = 50_000_000;
const VERY_LONG_BLOCKING_QUEUE_NANOS: u64 = 500_000_000;
const LONG_BLOCKING_EXECUTION_NANOS: u64 = 250_000_000;
const CACHE_LOOKUPS: u64 = 100;
const CACHE_MISS_PERCENT: u64 = 50;
const MAX_CATEGORY_FINDINGS: usize = 8;
const MAX_FINDINGS: usize = 20;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) enum Severity {
    Medium,
    High,
}

impl Severity {
    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Medium => "Medium",
            Self::High => "High",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) enum Category {
    Allocation,
    Contention,
    AsyncRuntime,
    BlockingRuntime,
    Cache,
    Io,
}

impl Category {
    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Allocation => "Allocation",
            Self::Contention => "Contention",
            Self::AsyncRuntime => "Async runtime",
            Self::BlockingRuntime => "Blocking runtime",
            Self::Cache => "Cache",
            Self::Io => "I/O",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Finding {
    pub(super) severity: Severity,
    pub(super) category: Category,
    pub(super) title: String,
    pub(super) evidence: String,
    pub(super) location: String,
    pub(super) explanation: String,
    pub(super) agent_instruction: String,
    pub(super) score: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct Report {
    pub(super) findings: Vec<Finding>,
    pub(super) capture_warning: Option<String>,
}

pub(super) fn analyze(
    allocations: Option<&AllocationSnapshot>,
    primitives: &PrimitiveSnapshot,
    runtime: &RuntimeMonitorSnapshot,
    io: &IoMonitorSnapshot,
    cache: &CacheMonitorSnapshot,
) -> Report {
    let mut findings = Vec::new();
    if let Some(allocations) = allocations {
        allocation_findings(allocations, &mut findings);
    }
    contention_findings(primitives, &mut findings);
    runtime_findings(runtime, &mut findings);
    cache_findings(cache, &mut findings);
    io_findings(io, &mut findings);
    findings.sort_unstable_by(compare_findings);
    findings.dedup_by(|left, right| left.category == right.category && left.title == right.title && left.location == right.location);
    findings.truncate(MAX_FINDINGS);

    let (total_events, lost_events) = allocations.map_or((primitives.total_events, primitives.lost_events), |allocations| {
        (allocations.total_events, allocations.lost_events)
    });
    let capture_warning = capture_warning(total_events, lost_events);
    Report { findings, capture_warning }
}

fn allocation_findings(allocations: &AllocationSnapshot, findings: &mut Vec<Finding>) {
    let mut candidates = allocations
        .hotspots
        .iter()
        .filter_map(|hotspot| {
            let location = hotspot.location(AllocationStackFilter::Application);
            let large = hotspot.max_allocation_bytes >= LARGE_ALLOCATION_BYTES;
            let churn = hotspot.allocations >= HIGH_ALLOCATION_COUNT && hotspot.allocated_bytes >= HIGH_ALLOCATED_BYTES;
            (location != "Backtraces disabled" && (large || churn)).then_some((
                hotspot,
                location,
                large,
                churn,
                hotspot.allocated_bytes.max(hotspot.max_allocation_bytes),
            ))
        })
        .collect::<Vec<_>>();
    candidates.sort_unstable_by_key(|(_, _, _, _, score)| std::cmp::Reverse(*score));
    for (hotspot, location, large, churn, score) in candidates.into_iter().take(MAX_CATEGORY_FINDINGS) {
        let severity = if hotspot.max_allocation_bytes >= VERY_LARGE_ALLOCATION_BYTES
            || hotspot.allocated_bytes >= HIGH_ALLOCATED_BYTES.saturating_mul(4)
        {
            Severity::High
        } else {
            Severity::Medium
        };
        let title = match (large, churn) {
            (true, true) => "Large repeated allocations",
            (true, false) => "Large allocation",
            (false, true) => "High allocation churn",
            (false, false) => unreachable!("guarded above"),
        };
        let evidence = format!(
            "{} allocations, {} total, {} maximum",
            format_count(hotspot.allocations),
            format_bytes(hotspot.allocated_bytes),
            format_bytes(hotspot.max_allocation_bytes)
        );
        let fix_direction = match (large, churn) {
            (true, true) => "Reuse storage across iterations and stream or chunk the largest temporary values.",
            (true, false) => "Reuse the large buffer when possible, or stream and process the value in bounded chunks.",
            (false, true) => "Reserve predictable capacity and reuse short-lived buffers or collections across iterations.",
            (false, false) => unreachable!("guarded above"),
        };
        findings.push(Finding {
            severity,
            category: Category::Allocation,
            title: title.into(),
            evidence: evidence.clone(),
            location: location.into(),
            explanation: "Repeated or large temporary allocations increase allocator work, memory traffic, and peak working set.".into(),
            agent_instruction: format!(
                "At {location}, reduce this allocation hotspot ({evidence}). {fix_direction} Preserve behavior and defensive checks. Add a \
                 focused test or benchmark, then compare allocation count, total bytes, and maximum allocation with the same workload."
            ),
            score,
        });
    }
}

fn contention_findings(primitives: &PrimitiveSnapshot, findings: &mut Vec<Finding>) {
    for group in &primitives.groups {
        for operation in group.operations.iter().filter(|operation| actionable_contention(operation.kind)) {
            if operation.events < CONTENTION_EVENTS {
                continue;
            }

            let Some(hotspot) = operation.hotspots.first() else {
                continue;
            };
            let location = hotspot.location(AllocationStackFilter::Application);
            if location == "Backtraces disabled" {
                continue;
            }
            let severity = if operation.events >= CONTENTION_EVENTS.saturating_mul(10) {
                Severity::High
            } else {
                Severity::Medium
            };
            let evidence = format!(
                "{} retained {} events across {} objects and {} threads; top stack {} events",
                format_count(operation.events),
                operation.kind.label(),
                format_count(operation.objects),
                format_count(operation.threads),
                format_count(hotspot.count)
            );
            findings.push(Finding {
                severity,
                category: Category::Contention,
                title: format!("{} contention", group.kind.label()),
                evidence: evidence.clone(),
                location: location.into(),
                explanation: "Frequent contention can serialize work and increase tail latency. Counts are retained events, not wait duration."
                    .into(),
                agent_instruction: format!(
                    "At {location}, reduce the observed {} ({evidence}). Shorten the critical section, move unrelated work outside the \
                     synchronization boundary, or partition shared state. Preserve synchronization semantics. Add a deterministic concurrency \
                     test and compare retained contention events with the same workload.",
                    operation.kind.label()
                ),
                score: operation.events,
            });
        }
    }
}

const fn actionable_contention(kind: super::data::PrimitiveOperationKind) -> bool {
    use super::data::PrimitiveOperationKind;
    matches!(
        kind,
        PrimitiveOperationKind::MutexContention
            | PrimitiveOperationKind::RwLockReadContention
            | PrimitiveOperationKind::RwLockWriteContention
            | PrimitiveOperationKind::OnceContention
            | PrimitiveOperationKind::ChannelSendContention
    )
}

fn runtime_findings(runtime: &RuntimeMonitorSnapshot, findings: &mut Vec<Finding>) {
    let mut asynchronous = runtime
        .asynchronous_groups
        .iter()
        .filter_map(|group| {
            let max_poll = group.poll_time.max_nanos?;
            let location = group.spawned_at.as_deref()?;
            (max_poll >= LONG_ASYNC_POLL_NANOS && (group.poll_samples.len() >= 3 || max_poll >= VERY_LONG_ASYNC_POLL_NANOS))
                .then_some((group, location, max_poll))
        })
        .collect::<Vec<_>>();
    asynchronous.sort_unstable_by_key(|(_, _, max_poll)| std::cmp::Reverse(*max_poll));
    for (group, location, max_poll) in asynchronous.into_iter().take(MAX_CATEGORY_FINDINGS) {
        let severity = if max_poll >= VERY_LONG_ASYNC_POLL_NANOS {
            Severity::High
        } else {
            Severity::Medium
        };
        let evidence = format!(
            "{} polls, {} average, {} maximum",
            format_count(group.polls),
            group.poll_time.mean_nanos().map_or_else(|| "-".into(), format_duration),
            format_duration(max_poll)
        );
        findings.push(Finding {
            severity,
            category: Category::AsyncRuntime,
            title: "Long async task polls".into(),
            evidence: evidence.clone(),
            location: location.into(),
            explanation: "A long poll occupies an async worker and delays unrelated ready tasks on that worker.".into(),
            agent_instruction: format!(
                "At {location}, shorten async poll execution ({evidence}). Move blocking or CPU-heavy work to the blocking pool, or split bounded \
                 work into smaller state-machine steps. Do not add unconditional yield loops that keep the same blocking work. Add a focused \
                 runtime test or benchmark and compare maximum poll duration."
            ),
            score: max_poll,
        });
    }

    let mut blocking = runtime
        .blocking_groups
        .iter()
        .filter_map(|group| {
            let max_queue = group.queue_time.max_nanos?;
            let location = group.spawned_at.as_deref()?;
            (max_queue >= LONG_BLOCKING_QUEUE_NANOS && group.executions >= 3).then_some((group, location, max_queue))
        })
        .collect::<Vec<_>>();
    blocking.sort_unstable_by_key(|(_, _, max_queue)| std::cmp::Reverse(*max_queue));
    for (group, location, max_queue) in blocking.into_iter().take(MAX_CATEGORY_FINDINGS) {
        let max_execution = group.execution_time.max_nanos.unwrap_or_default();
        let severity = if max_queue >= VERY_LONG_BLOCKING_QUEUE_NANOS || max_execution >= 1_000_000_000 {
            Severity::High
        } else {
            Severity::Medium
        };
        let evidence = format!(
            "{} runs, {} average queue, {} maximum queue, {} maximum execution",
            format_count(group.executions),
            group.queue_time.mean_nanos().map_or_else(|| "-".into(), format_duration),
            format_duration(max_queue),
            group.execution_time.max_nanos.map_or_else(|| "-".into(), format_duration)
        );
        let execution_advice = if max_execution >= LONG_BLOCKING_EXECUTION_NANOS {
            " Reduce callback duration or split large batches before increasing pool size."
        } else {
            " Reduce burst concurrency or batch submissions before increasing pool size."
        };
        findings.push(Finding {
            severity,
            category: Category::BlockingRuntime,
            title: "Blocking pool queue delay".into(),
            evidence: evidence.clone(),
            location: location.into(),
            explanation: "Blocking tasks waited before execution, which can delay every caller that depends on the shared pool.".into(),
            agent_instruction: format!(
                "At {location}, reduce blocking pool queue delay ({evidence}).{execution_advice} Preserve cancellation and error behavior. Add a \
                 focused load test and compare average and maximum queue duration."
            ),
            score: max_queue,
        });
    }
}

fn cache_findings(cache: &CacheMonitorSnapshot, findings: &mut Vec<Finding>) {
    let mut tiers = cache
        .tiers
        .iter()
        .filter(|tier| {
            let lookups = tier.hits.saturating_add(tier.misses).saturating_add(tier.errors);
            tier.errors != 0 || (lookups >= CACHE_LOOKUPS && tier.misses.saturating_mul(100) >= lookups.saturating_mul(CACHE_MISS_PERCENT))
        })
        .collect::<Vec<_>>();
    tiers.sort_unstable_by_key(|tier| std::cmp::Reverse((tier.errors, tier.misses)));
    for tier in tiers.into_iter().take(MAX_CATEGORY_FINDINGS) {
        let lookups = tier.hits.saturating_add(tier.misses).saturating_add(tier.errors);
        if tier.errors != 0 {
            findings.push(Finding {
                severity: Severity::High,
                category: Category::Cache,
                title: "Cache operation errors".into(),
                evidence: format!(
                    "{} errors across {} lookup outcomes",
                    format_count(tier.errors),
                    format_count(lookups)
                ),
                location: format!("cache tier 0x{:x}", tier.tier_id),
                explanation: "Cache errors can turn an optimization into request failures or repeated fallback work.".into(),
                agent_instruction: format!(
                    "Find the cache instrumentation for tier 0x{:x}. Fix the {} retained cache errors without hiding them or converting them to \
                     hits. Preserve fallback behavior. Add deterministic error and recovery tests, then compare the error count.",
                    tier.tier_id, tier.errors
                ),
                score: tier.errors,
            });
        }
        if lookups < CACHE_LOOKUPS || tier.misses.saturating_mul(100) < lookups.saturating_mul(CACHE_MISS_PERCENT) {
            continue;
        }
        let miss_percent = u128::from(tier.misses).saturating_mul(100) / u128::from(lookups);
        findings.push(Finding {
            severity: Severity::Medium,
            category: Category::Cache,
            title: "Low cache hit rate".into(),
            evidence: format!(
                "{} hits, {} misses, {} errors ({}% misses)",
                format_count(tier.hits),
                format_count(tier.misses),
                format_count(tier.errors),
                miss_percent
            ),
            location: format!("cache tier 0x{:x}", tier.tier_id),
            explanation: "A low hit rate adds compute, allocation, or I/O work while retaining cache maintenance overhead.".into(),
            agent_instruction: format!(
                "Find the cache instrumentation for tier 0x{:x}. Improve the observed hit rate by checking key normalization, capacity, lifetime, \
                 and invalidation behavior. Do not extend lifetime beyond correctness requirements. Add deterministic hit, miss, and eviction tests, \
                 then compare the same workload.",
                tier.tier_id
            ),
            score: tier.misses,
        });
    }
}

fn io_findings(io: &IoMonitorSnapshot, findings: &mut Vec<Finding>) {
    let mut resources = io.resources.iter().filter(|resource| resource.errors != 0).collect::<Vec<_>>();
    resources.sort_unstable_by_key(|resource| std::cmp::Reverse(resource.errors));
    for resource in resources.into_iter().take(MAX_CATEGORY_FINDINGS) {
        findings.push(Finding {
            severity: Severity::High,
            category: Category::Io,
            title: "I/O failures".into(),
            evidence: format!(
                "{} errors across {} reads and {} writes",
                format_count(resource.errors),
                format_count(resource.reads),
                format_count(resource.writes)
            ),
            location: format!("I/O resource #{}", resource.resource_id),
            explanation: "Failed I/O can trigger retries, partial work, and latency amplification.".into(),
            agent_instruction: format!(
                "Find the instrumentation for I/O resource #{}. Handle the {} retained errors explicitly. Retry only idempotent operations with \
                 bounded policy, preserve the original error, and add success, partial-completion, and failure tests.",
                resource.resource_id, resource.errors
            ),
            score: resource.errors,
        });
    }
}

fn capture_warning(total_events: u64, lost_events: u64) -> Option<String> {
    if total_events == 0 || u128::from(lost_events) * 100 < u128::from(total_events) * 5 {
        return None;
    }
    let tenths = u128::from(lost_events).saturating_mul(1_000) / u128::from(total_events);
    Some(format!(
        "{}.{:01}% of accepted telemetry was overwritten. Findings are retained lower-bound evidence.",
        tenths / 10,
        tenths % 10
    ))
}

fn compare_findings(left: &Finding, right: &Finding) -> Ordering {
    right
        .severity
        .cmp(&left.severity)
        .then_with(|| right.score.cmp(&left.score))
        .then_with(|| left.category.cmp(&right.category))
        .then_with(|| left.location.cmp(&right.location))
}

fn format_count(value: u64) -> String {
    let digits = value.to_string();
    let mut output = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, character) in digits.chars().enumerate() {
        if index != 0 && (digits.len() - index).is_multiple_of(3) {
            output.push(',');
        }
        output.push(character);
    }
    output
}

fn format_bytes(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = KIB * 1024;
    const GIB: u64 = MIB * 1024;
    if bytes >= GIB {
        format_decimal(bytes, GIB, "GiB")
    } else if bytes >= MIB {
        format_decimal(bytes, MIB, "MiB")
    } else if bytes >= KIB {
        format_decimal(bytes, KIB, "KiB")
    } else {
        format!("{bytes} B")
    }
}

fn format_duration(nanos: u64) -> String {
    if nanos >= 1_000_000_000 {
        format_decimal(nanos, 1_000_000_000, "s")
    } else if nanos >= 1_000_000 {
        format_decimal(nanos, 1_000_000, "ms")
    } else if nanos >= 1_000 {
        format_decimal(nanos, 1_000, "us")
    } else {
        format!("{nanos}ns")
    }
}

fn format_decimal(value: u64, unit: u64, suffix: &str) -> String {
    let whole = value / unit;
    let hundredths = value % unit * 100 / unit;
    format!("{whole}.{hundredths:02} {suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::monitor::data::{CacheTierSummary, IoResourceSummary};
    use crate::commands::monitor::runtime::{BlockingTaskGroupSummary, DurationSummary};

    fn primitives(total_events: u64, lost_events: u64) -> PrimitiveSnapshot {
        PrimitiveSnapshot {
            total_events,
            lost_events,
            groups: Vec::new(),
        }
    }

    #[test]
    fn blocking_queue_and_cache_miss_findings_include_agent_instructions() {
        let runtime = RuntimeMonitorSnapshot {
            blocking_groups: vec![BlockingTaskGroupSummary {
                spawned_at: Some("src/service.rs:42:9".into()),
                executions: 8,
                queue_time: DurationSummary {
                    samples: 8,
                    total_nanos: 4_000_000_000,
                    max_nanos: Some(900_000_000),
                },
                execution_time: DurationSummary {
                    samples: 8,
                    total_nanos: 8_000_000_000,
                    max_nanos: Some(1_200_000_000),
                },
                ..BlockingTaskGroupSummary::default()
            }],
            ..RuntimeMonitorSnapshot::default()
        };
        let cache = CacheMonitorSnapshot {
            total_events: 200,
            retained_events: 200,
            lost_events: 0,
            tiers: vec![CacheTierSummary {
                tier_id: 7,
                fallback: false,
                events: 200,
                hits: 40,
                misses: 160,
                errors: 0,
                operations: Vec::new(),
            }],
        };

        let report = analyze(None, &primitives(200, 0), &runtime, &IoMonitorSnapshot::default(), &cache);

        assert_eq!(report.findings.len(), 2);
        assert!(report.findings[0].agent_instruction.contains("focused load test"));
        assert!(report.findings[1].agent_instruction.contains("key normalization"));
        assert!(report.findings.iter().all(|finding| !finding.location.is_empty()));
    }

    #[test]
    fn capture_warning_does_not_turn_loss_into_an_application_hotspot() {
        let report = analyze(
            None,
            &primitives(100, 25),
            &RuntimeMonitorSnapshot::default(),
            &IoMonitorSnapshot::default(),
            &CacheMonitorSnapshot::default(),
        );

        assert!(report.findings.is_empty());
        assert_eq!(
            report.capture_warning.as_deref(),
            Some("25.0% of accepted telemetry was overwritten. Findings are retained lower-bound evidence.")
        );
    }

    #[test]
    fn normal_coordination_waits_are_not_actionable_contention() {
        use crate::commands::monitor::data::PrimitiveOperationKind;

        assert!(!actionable_contention(PrimitiveOperationKind::NotifyContention));
        assert!(!actionable_contention(PrimitiveOperationKind::CondvarContention));
        assert!(actionable_contention(PrimitiveOperationKind::MutexContention));
    }

    #[test]
    fn canceled_io_without_errors_is_not_reported_as_a_hotspot() {
        let io = IoMonitorSnapshot {
            resources: vec![IoResourceSummary {
                resource_id: 1,
                kind: seismograph::recorder::io::IoResourceKind::File,
                events: 2,
                reads: 1,
                writes: 0,
                completed: 1,
                errors: 0,
                canceled: 1,
                requested_bytes: 10,
                completed_bytes: 0,
                operations: Vec::new(),
            }],
            ..IoMonitorSnapshot::default()
        };

        let report = analyze(
            None,
            &primitives(2, 0),
            &RuntimeMonitorSnapshot::default(),
            &io,
            &CacheMonitorSnapshot::default(),
        );

        assert!(report.findings.is_empty());
    }
}
