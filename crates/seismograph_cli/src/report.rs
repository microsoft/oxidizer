// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    reason = "Report coordinates, percentages, and color values are intentionally lossy display-only conversions"
)]
#![expect(
    clippy::too_many_lines,
    reason = "HTML sections remain linear to keep the generated document structure auditable"
)]
#![expect(clippy::unwrap_used, reason = "Writing formatted text into String is infallible")]

//! Snapshot-to-HTML reporting for the `rallocator` command.

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;
use std::sync::Arc;

use seismograph::recorder::event::EventKind as RuntimeEventKind;
use seismograph_rallocator::callers::{AddressLookup, Callers, Event, EventKind, HeapKind};
use seismograph_rallocator::snapshot::{Domain, Estimate, Snapshot};
use seismograph_rallocator::topology::{Slice, SliceKind, TopologyRegion};

const SNAPSHOT_TEMPLATE: &str = include_str!("templates/snapshot.html");
#[cfg(test)]
pub(crate) fn render_html(snapshot: &Snapshot) -> String {
    render_html_with_sources(snapshot, &[])
}

pub(crate) fn render_html_with_sources(snapshot: &Snapshot, sources: &[seismograph::snapshot::SourceSnapshot]) -> String {
    let stats = snapshot.stats;
    let mut html = String::with_capacity(64 * 1024);
    write!(
        html,
        r#"<div class="hero"><div><h1>seismograph snapshot</h1><div class="subtitle">Process telemetry captured in a portable binary snapshot</div></div>
<div class="version">wire {}{} · schema {}{}<br>producer {}.{}.{}{}</div></div>
<div class="grid">
<div class="card"><div class="label">Live requested{}</div><div class="metric">{}</div></div>
<div class="card"><div class="label">Committed{}</div><div class="metric">{}</div></div>
<div class="card"><div class="label">Allocations{}</div><div class="metric">{}</div></div>
<div class="card"><div class="label">Remote frees{}</div><div class="metric">{}</div></div>
</div>"#,
        snapshot.metadata.wire_format_version,
        info("Version of the stable binary container and primitive encoding."),
        snapshot.metadata.telemetry_schema_version,
        info("Version of the telemetry records stored inside the wire container."),
        snapshot.metadata.producer_version.major,
        snapshot.metadata.producer_version.minor,
        snapshot.metadata.producer_version.patch,
        info("Version of rallocator that created this snapshot."),
        info("Bytes requested by allocations that were live when the snapshot was taken."),
        format_bytes(stats.live_bytes),
        info("Memory currently committed or mapped for allocator use, including retained backing."),
        format_bytes(stats.mapped_bytes),
        info("Total successful allocation operations recorded since process start."),
        format_count(stats.allocations),
        info("Deallocations performed by a thread other than the allocation's owning thread."),
        format_count(stats.remote_frees),
    )
    .unwrap();

    if !snapshot.skipped_sections.is_empty() {
        html.push_str("<section><h2>Compatibility warning</h2><p>This report omitted snapshot sections with unknown identifiers or versions unsupported by this decoder: ");
        for (index, section) in snapshot.skipped_sections.iter().enumerate() {
            if index != 0 {
                html.push_str(", ");
            }
            write!(html, "{} (version {})", section.id, section.version).unwrap();
        }
        html.push_str(". Use a compatible seismograph version to inspect the omitted data.</p></section>");
    }

    render_sources(&mut html, sources);
    render_domains(&mut html, snapshot);
    render_allocation_histograms(&mut html, snapshot);
    render_physical_topology(&mut html, snapshot);
    render_allocator_structures(&mut html, snapshot);
    render_size_classes(&mut html, snapshot);
    render_runtime_events(&mut html, snapshot);

    write!(
        html,
        r"<section><h2>{}</h2><table>
<tr><th>Metric</th><th>Value</th></tr>
<tr><td>{}</td><td>{}</td></tr>
<tr><td>{}</td><td>{}</td></tr>
<tr><td>{}</td><td>{}</td></tr>
<tr><td>{}</td><td>{:.3} ms</td></tr>
<tr><td>{}</td><td>{} / {}</td></tr>
<tr><td>{}</td><td>{} / {}</td></tr>
</table></section>",
        concept(
            "Process totals",
            "Process-wide counters accumulated by the telemetry-enabled allocator."
        ),
        concept(
            "Live requested bytes",
            "The sum of application-requested sizes for allocations still live at capture time."
        ),
        format_bytes(stats.live_bytes),
        concept(
            "Peak live bytes",
            "The highest live requested-byte total observed since process start."
        ),
        format_bytes(stats.peak_live_bytes),
        concept(
            "Committed / mapped bytes",
            "Memory made accessible by the operating system for allocator data and retained backing."
        ),
        format_bytes(stats.mapped_bytes),
        concept(
            "Snapshot capture time",
            "Wall-clock time spent collecting and encoding this snapshot."
        ),
        snapshot.metadata.capture_duration_nanos as f64 / 1_000_000.0,
        concept(
            "Allocations",
            "Recorded allocation operation count followed by the cumulative requested bytes."
        ),
        format_count(stats.allocations),
        format_bytes(stats.allocated_bytes),
        concept(
            "Deallocations",
            "Recorded deallocation operation count followed by the cumulative requested bytes released."
        ),
        format_count(stats.deallocations),
        format_bytes(stats.deallocated_bytes),
    )
    .unwrap();

    write!(
        html,
        r"<section><h2>{}</h2><table><tr><th>Metric</th><th>Value</th></tr>
<tr><td>{}</td><td>{}</td></tr><tr><td>{}</td><td>{}</td></tr>
<tr><td>{}</td><td>{}</td></tr><tr><td>{}</td><td>{}</td></tr>
<tr><td>{}</td><td>{}</td></tr></table></section>",
        concept(
            "Remote frees and reclamation",
            "Cross-thread deallocation queues and operating-system memory mapping activity."
        ),
        concept(
            "Remote frees",
            "Blocks freed by a thread other than the one owning their heap or slab."
        ),
        format_count(stats.remote_frees),
        concept(
            "Pending remote blocks",
            "Remotely freed blocks queued for processing by their owning heap."
        ),
        format_count(stats.pending_remote_blocks),
        concept(
            "Drained remote blocks",
            "Queued remote blocks already reclaimed by their owning heap."
        ),
        format_count(stats.drained_remote_blocks),
        concept(
            "OS mappings",
            "Successful requests that mapped or committed memory through the allocator HAL."
        ),
        format_count(stats.os_mappings),
        concept(
            "OS unmappings / reclamations",
            "Allocator requests that returned or decommitted memory through the allocator HAL."
        ),
        format_count(stats.os_unmappings),
    )
    .unwrap();

    write!(
        html,
        "<section><details><summary>{}</summary>",
        concept(
            "Retained caller stacks",
            "Call stacks grouped by allocation site for unmatched allocations retained in the bounded event log."
        )
    )
    .unwrap();
    match snapshot.callers.as_ref() {
        Some(callers) => {
            render_hotspots(&mut html, callers, &snapshot.addresses);
            render_thread_histograms(&mut html, callers);
            render_callers(&mut html, callers, &snapshot.addresses);
        }

        None => html.push_str("<p class=\"muted\">Caller tracking was not available for this snapshot.</p>"),
    }
    html.push_str("</details></section>");
    SNAPSHOT_TEMPLATE.replace("{{REPORT_BODY}}", &html)
}

fn render_sources(html: &mut String, sources: &[seismograph::snapshot::SourceSnapshot]) {
    if sources.is_empty() {
        return;
    }
    html.push_str("<section><h2>Snapshot sources</h2><table><tr><th>Source</th><th>Schema</th><th>Payload</th><th>Rendering</th></tr>");
    for source in sources {
        let rendering = if source.id == seismograph_rallocator::source::ID {
            "Specialized allocator panels"
        } else {
            "Generic source metadata only"
        };
        write!(
            html,
            "<tr><td>{}</td><td>{}</td><td>{}</td><td>{rendering}</td></tr>",
            escape_html(&source.name),
            source.schema_version,
            format_bytes(source.data.len() as u64),
        )
        .unwrap();
    }
    html.push_str("</table></section>");
}

fn render_runtime_events(html: &mut String, snapshot: &Snapshot) {
    write!(
        html,
        "<section class=\"runtime-telemetry\"><details open><summary>{}</summary>",
        concept(
            "Runtime ownership and lock telemetry",
            "Instrumented Arc and synchronization operations captured by performables, including optional call stacks and allocator correlation."
        )
    )
    .unwrap();

    let Some(runtime_events) = snapshot.runtime_events.as_ref() else {
        html.push_str("<p class=\"muted\">Runtime ownership and lock telemetry was not enabled for this snapshot.</p></details></section>");
        return;
    };

    let address_lookups = snapshot
        .addresses
        .iter()
        .map(|lookup| (lookup.address, lookup))
        .collect::<HashMap<_, _>>();

    write!(
        html,
        "<p class=\"section-note\">{} retained events from {} recording threads; {} events were overwritten before capture. Select any graph item to inspect its call sites, sorted by retained event count.</p>",
        format_count(runtime_events.events.len() as u64),
        format_count(runtime_events.threads.len() as u64),
        format_count(runtime_events.lost_events),
    )
    .unwrap();
    let sampled_policies = [
        ("allocation", runtime_events.recording.allocations),
        ("general event", runtime_events.recording.general_events),
        ("Arc dereference", runtime_events.recording.arc_dereferences),
        ("runtime task", runtime_events.recording.runtime_tasks),
        ("I/O", runtime_events.recording.io),
    ];
    for (label, policy) in sampled_policies {
        if policy.event_sampling.get() <= 1 {
            continue;
        }
        write!(
            html,
            "<p class=\"section-note\">{label} sampling recorded one in {} objects. Event counts below describe sampled objects and are not population estimates.</p>",
            format_count(u64::try_from(policy.event_sampling.get()).unwrap_or(u64::MAX)),
        )
        .unwrap();
    }
    let arc_kinds = [
        RuntimeEventKind::ArcCreate,
        RuntimeEventKind::ArcClone,
        RuntimeEventKind::ArcDeref,
        RuntimeEventKind::ArcDrop,
        RuntimeEventKind::ArcRelocate,
    ];
    let mutex_identity_kinds = [
        RuntimeEventKind::MutexAccess,
        RuntimeEventKind::MutexContention,
        RuntimeEventKind::MutexRelease,
    ];
    let mutex_kinds = [
        RuntimeEventKind::MutexAccess,
        RuntimeEventKind::MutexContention,
        RuntimeEventKind::MutexRelease,
        RuntimeEventKind::LockPoisoned,
        RuntimeEventKind::LockPoisonObserved,
        RuntimeEventKind::LockPoisonCleared,
    ];
    let rw_lock_identity_kinds = [
        RuntimeEventKind::RwLockReadAccess,
        RuntimeEventKind::RwLockReadContention,
        RuntimeEventKind::RwLockReadRelease,
        RuntimeEventKind::RwLockWriteAccess,
        RuntimeEventKind::RwLockWriteContention,
        RuntimeEventKind::RwLockWriteRelease,
    ];
    let rw_lock_kinds = [
        RuntimeEventKind::RwLockReadAccess,
        RuntimeEventKind::RwLockReadContention,
        RuntimeEventKind::RwLockReadRelease,
        RuntimeEventKind::RwLockWriteAccess,
        RuntimeEventKind::RwLockWriteContention,
        RuntimeEventKind::RwLockWriteRelease,
        RuntimeEventKind::LockPoisoned,
        RuntimeEventKind::LockPoisonObserved,
        RuntimeEventKind::LockPoisonCleared,
    ];
    let barrier_kinds = [
        RuntimeEventKind::BarrierAccess,
        RuntimeEventKind::BarrierContention,
        RuntimeEventKind::BarrierRelease,
    ];
    let condvar_kinds = [
        RuntimeEventKind::CondvarAccess,
        RuntimeEventKind::CondvarContention,
        RuntimeEventKind::CondvarNotify,
    ];
    let once_kinds = [
        RuntimeEventKind::OnceAccess,
        RuntimeEventKind::OnceContention,
        RuntimeEventKind::OnceInitialize,
    ];
    let channel_kinds = [
        RuntimeEventKind::ChannelSend,
        RuntimeEventKind::ChannelSendContention,
        RuntimeEventKind::ChannelReceive,
        RuntimeEventKind::ChannelReceiveContention,
        RuntimeEventKind::ChannelClose,
        RuntimeEventKind::ChannelHighWatermark,
    ];

    let arc_metrics = runtime_object_metrics(
        "arc",
        "Arc",
        &runtime_events.events,
        &arc_kinds,
        &arc_kinds,
        &[RuntimeEventKind::ArcDeref],
    );
    let mutex_metrics = runtime_object_metrics(
        "mutex",
        "Mutex",
        &runtime_events.events,
        &mutex_kinds,
        &mutex_identity_kinds,
        &[RuntimeEventKind::MutexAccess],
    );
    let rw_lock_metrics = runtime_object_metrics(
        "rwlock",
        "RwLock",
        &runtime_events.events,
        &rw_lock_kinds,
        &rw_lock_identity_kinds,
        &[RuntimeEventKind::RwLockReadAccess, RuntimeEventKind::RwLockWriteAccess],
    );
    let barrier_metrics = runtime_object_metrics(
        "barrier",
        "Barrier",
        &runtime_events.events,
        &barrier_kinds,
        &barrier_kinds,
        &[RuntimeEventKind::BarrierAccess],
    );
    let condvar_metrics = runtime_object_metrics(
        "condvar",
        "Condvar",
        &runtime_events.events,
        &condvar_kinds,
        &condvar_kinds,
        &[RuntimeEventKind::CondvarAccess],
    );
    let once_metrics = runtime_object_metrics(
        "once",
        "Once",
        &runtime_events.events,
        &once_kinds,
        &once_kinds,
        &[RuntimeEventKind::OnceAccess],
    );
    let channel_metrics = runtime_object_metrics(
        "channel",
        "Channel",
        &runtime_events.events,
        &channel_kinds,
        &channel_kinds,
        &[RuntimeEventKind::ChannelReceive],
    );

    html.push_str("<div class=\"runtime-clusters\">");
    render_runtime_cluster(
        html,
        "Arc",
        &format!("{} observed objects, ranked by dereferences.", arc_metrics.len()),
        &arc_metrics,
    );
    render_runtime_cluster(
        html,
        "Mutex",
        &format!("{} observed objects, ranked by acquisitions.", mutex_metrics.len()),
        &mutex_metrics,
    );
    render_runtime_cluster(
        html,
        "RwLock",
        &format!("{} observed objects, ranked by read/write acquisitions.", rw_lock_metrics.len()),
        &rw_lock_metrics,
    );
    render_runtime_cluster(
        html,
        "Barrier",
        &format!("{} observed objects, ranked by completed waits.", barrier_metrics.len()),
        &barrier_metrics,
    );
    render_runtime_cluster(
        html,
        "Condvar",
        &format!("{} observed objects, ranked by completed waits.", condvar_metrics.len()),
        &condvar_metrics,
    );
    render_runtime_cluster(
        html,
        "OnceLock / LazyLock",
        &format!("{} observed objects, ranked by accesses.", once_metrics.len()),
        &once_metrics,
    );
    render_runtime_cluster(
        html,
        "Channel",
        &format!("{} observed objects, ranked by receives.", channel_metrics.len()),
        &channel_metrics,
    );
    html.push_str("</div><div class=\"runtime-stack-explorer\"><p class=\"runtime-stack-placeholder\">Select a graph item to inspect its hottest retained call stacks.</p>");
    for metric in arc_metrics
        .iter()
        .chain(mutex_metrics.iter())
        .chain(rw_lock_metrics.iter())
        .chain(barrier_metrics.iter())
        .chain(condvar_metrics.iter())
        .chain(once_metrics.iter())
        .chain(channel_metrics.iter())
    {
        render_runtime_stack_panel(html, metric, &address_lookups);
    }
    html.push_str("</div></details></section>");
}

struct RuntimeMetric {
    id: String,
    label: String,
    value: String,
    magnitude: u64,
    detail: String,
    threads: BTreeMap<u64, u64>,
    stacks: Vec<RuntimeStackTotal>,
    contention_pairs: Vec<RuntimeContentionPair>,
}

struct RuntimeStackTotal {
    kind: String,
    stack: Vec<u64>,
    count: u64,
}

struct RuntimeContentionPair {
    kind: String,
    blocked: Vec<u64>,
    holder: Option<Vec<u64>>,
    count: u64,
}

fn render_runtime_cluster(html: &mut String, title: &str, description: &str, metrics: &[RuntimeMetric]) {
    let maximum = metrics.iter().map(|metric| metric.magnitude).max().unwrap_or(1);
    write!(
        html,
        "<article class=\"runtime-cluster\"><h3>{}</h3><p class=\"muted\">{}</p><div class=\"runtime-metrics\">",
        escape_html(title),
        escape_html(description),
    )
    .unwrap();
    for metric in metrics {
        let width = if maximum == 0 {
            0.0
        } else {
            100.0 * metric.magnitude as f64 / maximum as f64
        };
        write!(
            html,
            "<button type=\"button\" class=\"runtime-metric\" data-runtime-panel=\"{}\" aria-controls=\"{}\"><span class=\"runtime-metric-heading\"><span>{}</span><strong>{}</strong></span><span class=\"runtime-metric-bar\"><i style=\"width:{width:.2}%\"></i></span><span class=\"runtime-metric-detail\">{}</span>",
            metric.id,
            metric.id,
            escape_html(&metric.label),
            escape_html(&metric.value),
            escape_html(&metric.detail),
        )
        .unwrap();
        render_runtime_thread_bar(html, &metric.threads);
        html.push_str("</button>");
    }
    html.push_str("</div></article>");
}

fn render_runtime_stack_panel(html: &mut String, metric: &RuntimeMetric, lookups: &HashMap<u64, &AddressLookup>) {
    write!(
        html,
        "<div class=\"runtime-stack-panel\" id=\"{}\" hidden><h3>{} call sites</h3>",
        metric.id,
        escape_html(&metric.label),
    )
    .unwrap();
    if metric.stacks.is_empty() {
        html.push_str("<p class=\"muted\">No retained call stacks for this graph item.</p></div>");
        return;
    }
    if !metric.contention_pairs.is_empty() {
        html.push_str("<h4>Contention parties</h4><div class=\"runtime-contention-pairs\">");
        for pair in metric.contention_pairs.iter().take(20) {
            write!(
                html,
                "<article class=\"runtime-contention-pair\"><h5>{} · {} observation{}</h5><div><strong>Blocked acquisition</strong>{}</div><div><strong>Current holder</strong>{}</div></article>",
                escape_html(&pair.kind),
                format_count(pair.count),
                if pair.count == 1 { "" } else { "s" },
                render_runtime_party_stack(&pair.blocked, lookups),
                pair.holder
                    .as_ref()
                    .map_or_else(|| "<p class=\"muted\">Holder stack unavailable.</p>".to_owned(), |stack| {
                        render_runtime_party_stack(stack, lookups)
                    }),
            )
            .unwrap();
        }
        html.push_str("</div>");
    }
    html.push_str("<h4>Operation call sites</h4><div class=\"runtime-stack-list\">");
    let total = metric.stacks.iter().map(|entry| entry.count).sum::<u64>();
    for entry in metric.stacks.iter().take(100) {
        let frames = runtime_stack_frames(&entry.stack, lookups);
        let summary = frames.first().map_or("No captured stack", String::as_str);
        write!(
            html,
            "<details class=\"runtime-stack-entry\"><summary><strong>{} × {}</strong><span>{}</span><small>{:.2}%</small></summary>{}</details>",
            format_count(entry.count),
            escape_html(&entry.kind),
            escape_html(summary),
            percentage(entry.count, total),
            render_runtime_frames(&frames),
        )
        .unwrap();
    }
    html.push_str("</div></div>");
}

fn runtime_object_metrics(
    id_prefix: &str,
    label: &str,
    events: &[seismograph::recorder::event::Event],
    kinds: &[RuntimeEventKind],
    identity_kinds: &[RuntimeEventKind],
    primary_kinds: &[RuntimeEventKind],
) -> Vec<RuntimeMetric> {
    let object_ids = events
        .iter()
        .filter(|event| identity_kinds.contains(&event.kind))
        .filter_map(seismograph::recorder::event::Event::object_id)
        .collect::<HashSet<_>>();
    let mut objects = object_ids
        .into_iter()
        .map(|object_id| {
            let object_events = events
                .iter()
                .filter(|event| event.object_id() == Some(object_id) && kinds.contains(&event.kind))
                .collect::<Vec<_>>();
            let magnitude = object_events.iter().filter(|event| primary_kinds.contains(&event.kind)).count() as u64;
            let mut threads = BTreeMap::<u64, u64>::new();
            let mut stack_totals = HashMap::<(String, Vec<u64>), u64>::new();
            for event in &object_events {
                *threads.entry(event.thread_id.get()).or_default() += 1;
                *stack_totals.entry((format!("{:?}", event.kind), runtime_stack(event))).or_default() += 1;
            }
            let mut stacks = stack_totals
                .into_iter()
                .map(|((kind, stack), count)| RuntimeStackTotal { kind, stack, count })
                .collect::<Vec<_>>();
            stacks.sort_unstable_by_key(|entry| std::cmp::Reverse(entry.count));
            let detail = runtime_object_detail(id_prefix, &object_events, threads.len());
            RuntimeMetric {
                id: format!("runtime-{id_prefix}-{:016x}", object_id.get()),
                label: format!("{label} 0x{:016x}", object_id.get()),
                value: match id_prefix {
                    "arc" => format!("{} derefs", format_count(magnitude)),
                    "mutex" | "rwlock" => format!("{} locks", format_count(magnitude)),
                    _ => format_count(magnitude),
                },
                magnitude,
                detail,
                threads,
                stacks,
                contention_pairs: runtime_contention_pairs(&object_events),
            }
        })
        .collect::<Vec<_>>();
    objects.sort_unstable_by(|left, right| {
        right.magnitude.cmp(&left.magnitude).then_with(|| {
            right
                .stacks
                .iter()
                .map(|entry| entry.count)
                .sum::<u64>()
                .cmp(&left.stacks.iter().map(|entry| entry.count).sum::<u64>())
        })
    });
    for (index, object) in objects.iter_mut().enumerate() {
        object.label = format!("#{} · {}", index + 1, object.label);
    }
    objects
}

fn runtime_object_detail(id_prefix: &str, events: &[&seismograph::recorder::event::Event], thread_count: usize) -> String {
    let count = |kind| events.iter().filter(|event| event.kind == kind).count() as u64;
    let threads = format!(
        "{} thread{}",
        format_count(thread_count as u64),
        if thread_count == 1 { "" } else { "s" }
    );
    match id_prefix {
        "arc" => format!(
            "{} derefs · {} clones · {} relocations · {threads}",
            format_count(count(RuntimeEventKind::ArcDeref)),
            format_count(count(RuntimeEventKind::ArcClone)),
            format_count(count(RuntimeEventKind::ArcRelocate)),
        ),
        "mutex" => format!(
            "{} locks · {} contentions · {} poisonings · {} observations · {} clears · {threads}",
            format_count(count(RuntimeEventKind::MutexAccess)),
            format_count(count(RuntimeEventKind::MutexContention)),
            format_count(count(RuntimeEventKind::LockPoisoned)),
            format_count(count(RuntimeEventKind::LockPoisonObserved)),
            format_count(count(RuntimeEventKind::LockPoisonCleared)),
        ),
        "rwlock" => format!(
            "{} reads · {} writes · {} contentions · {} poisonings · {} observations · {} clears · {threads}",
            format_count(count(RuntimeEventKind::RwLockReadAccess)),
            format_count(count(RuntimeEventKind::RwLockWriteAccess)),
            format_count(count(RuntimeEventKind::RwLockReadContention) + count(RuntimeEventKind::RwLockWriteContention)),
            format_count(count(RuntimeEventKind::LockPoisoned)),
            format_count(count(RuntimeEventKind::LockPoisonObserved)),
            format_count(count(RuntimeEventKind::LockPoisonCleared)),
        ),
        "barrier" => format!(
            "{} waits · {} blocked · {} releases · {threads}",
            format_count(count(RuntimeEventKind::BarrierAccess)),
            format_count(count(RuntimeEventKind::BarrierContention)),
            format_count(count(RuntimeEventKind::BarrierRelease)),
        ),
        "condvar" => format!(
            "{} waits · {} blocked · {} notifications · {threads}",
            format_count(count(RuntimeEventKind::CondvarAccess)),
            format_count(count(RuntimeEventKind::CondvarContention)),
            format_count(count(RuntimeEventKind::CondvarNotify)),
        ),
        "once" => format!(
            "{} accesses · {} initializations · {} contentions · {threads}",
            format_count(count(RuntimeEventKind::OnceAccess)),
            format_count(count(RuntimeEventKind::OnceInitialize)),
            format_count(count(RuntimeEventKind::OnceContention)),
        ),
        "channel" => format!(
            "{} sends · {} receives · {} contentions · high watermark {} · {} closes · {threads}",
            format_count(count(RuntimeEventKind::ChannelSend)),
            format_count(count(RuntimeEventKind::ChannelReceive)),
            format_count(count(RuntimeEventKind::ChannelSendContention) + count(RuntimeEventKind::ChannelReceiveContention)),
            format_count(
                events
                    .iter()
                    .filter(|event| event.kind == RuntimeEventKind::ChannelHighWatermark)
                    .filter_map(|event| event.measurement())
                    .max()
                    .unwrap_or(0),
            ),
            format_count(count(RuntimeEventKind::ChannelClose)),
        ),
        _ => threads,
    }
}

fn render_runtime_thread_bar(html: &mut String, threads: &BTreeMap<u64, u64>) {
    let total = threads.values().sum::<u64>();
    write!(
        html,
        "<span class=\"runtime-thread-heading\">Thread participation: {} thread{}</span><span class=\"runtime-thread-bar\">",
        format_count(threads.len() as u64),
        if threads.len() == 1 { "" } else { "s" },
    )
    .unwrap();
    for (index, (thread_id, events)) in threads.iter().enumerate() {
        let width = percentage(*events, total);
        let hue = (index * 67 + 195) % 360;
        write!(
            html,
            "<i style=\"width:{width:.2}%;background:hsl({hue} 70% 60%)\" title=\"thread #{thread_id}: {} events\"></i>",
            format_count(*events),
        )
        .unwrap();
    }
    html.push_str("</span>");
}

fn runtime_contention_pairs(events: &[&seismograph::recorder::event::Event]) -> Vec<RuntimeContentionPair> {
    let threads = events.iter().map(|event| event.thread_id).collect::<HashSet<_>>();
    let ordered = threads.len() == 1;
    let mut events = events.to_vec();
    if ordered {
        events.sort_unstable_by_key(|event| event.sequence);
    }
    let mut mutex_holder = None;
    let mut rw_writer = None;
    let mut rw_readers = Vec::<(seismograph::recorder::thread::ThreadId, Vec<u64>)>::new();
    let mut pairs = HashMap::<(String, Vec<u64>, Option<Vec<u64>>), u64>::new();
    for event in events {
        match event.kind {
            RuntimeEventKind::MutexAccess => mutex_holder = Some(runtime_stack(event)),
            RuntimeEventKind::MutexRelease => mutex_holder = None,
            RuntimeEventKind::MutexContention => {
                let holder = ordered.then(|| mutex_holder.clone()).flatten();
                *pairs
                    .entry(("Mutex contention".to_owned(), runtime_stack(event), holder))
                    .or_default() += 1;
            }
            RuntimeEventKind::RwLockReadAccess => {
                rw_readers.push((event.thread_id, runtime_stack(event)));
            }
            RuntimeEventKind::RwLockReadRelease => {
                if let Some(index) = rw_readers.iter().rposition(|(thread_id, _)| *thread_id == event.thread_id) {
                    rw_readers.remove(index);
                }
            }
            RuntimeEventKind::RwLockWriteAccess => rw_writer = Some(runtime_stack(event)),
            RuntimeEventKind::RwLockWriteRelease => rw_writer = None,
            RuntimeEventKind::RwLockReadContention => {
                let holder = ordered.then(|| rw_writer.clone()).flatten();
                *pairs
                    .entry(("Read contention".to_owned(), runtime_stack(event), holder))
                    .or_default() += 1;
            }
            RuntimeEventKind::RwLockWriteContention => {
                let holder = if ordered {
                    rw_writer.clone().or_else(|| rw_readers.last().map(|(_, stack)| stack.clone()))
                } else {
                    None
                };
                *pairs
                    .entry(("Write contention".to_owned(), runtime_stack(event), holder))
                    .or_default() += 1;
            }
            _ => {}
        }
    }
    let mut pairs = pairs
        .into_iter()
        .map(|((kind, blocked, holder), count)| RuntimeContentionPair {
            kind,
            blocked,
            holder,
            count,
        })
        .collect::<Vec<_>>();
    pairs.sort_unstable_by_key(|pair| std::cmp::Reverse(pair.count));
    pairs
}

fn runtime_stack(event: &seismograph::recorder::event::Event) -> Vec<u64> {
    event.call_stack.iter().map(|address| address.get()).collect()
}

fn ratio(numerator: u64, denominator: u64) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        numerator as f64 / denominator as f64
    }
}

fn percentage(numerator: u64, denominator: u64) -> f64 {
    100.0 * ratio(numerator, denominator)
}

fn render_runtime_party_stack(stack: &[u64], lookups: &HashMap<u64, &AddressLookup>) -> String {
    let frames = runtime_stack_frames(stack, lookups);
    let Some(summary) = frames.first() else {
        return "<p class=\"muted\">No captured stack.</p>".to_owned();
    };
    format!(
        "<p class=\"runtime-party-top\"><code>{}</code></p><details class=\"runtime-party-stack\"><summary>Full stack ({} frames)</summary>{}</details>",
        escape_html(summary),
        frames.len(),
        render_runtime_frames(&frames),
    )
}

fn runtime_stack_frames(stack: &[u64], lookups: &HashMap<u64, &AddressLookup>) -> Vec<String> {
    let frames = stack
        .iter()
        .map(|address| {
            let lookup = lookups.get(address).copied();
            let full_symbol = lookup.and_then(|lookup| lookup.symbol.as_deref()).unwrap_or_default();
            let filename = lookup.and_then(|lookup| lookup.filename.as_deref()).unwrap_or_default();
            (
                is_runtime_recorder_frame(full_symbol)
                    || filename.contains("performables\\src\\sync")
                    || filename.contains("performables/src/sync"),
                format_runtime_frame(*address, lookup),
            )
        })
        .collect::<Vec<_>>();
    let first_relevant = frames.iter().rposition(|(internal, _)| *internal).map_or(0, |index| index + 1);
    frames.into_iter().skip(first_relevant).take(12).map(|(_, frame)| frame).collect()
}

fn render_runtime_frames(frames: &[String]) -> String {
    if frames.is_empty() {
        return "<p class=\"muted\">No captured stack.</p>".to_owned();
    }
    let mut html = String::from("<ol class=\"stack-details\">");
    for frame in frames {
        write!(html, "<li><code>{}</code></li>", escape_html(frame)).unwrap();
    }
    html.push_str("</ol>");
    html
}

fn format_runtime_frame(address: u64, lookup: Option<&AddressLookup>) -> String {
    let Some(lookup) = lookup else {
        return format!("0x{address:016x}");
    };
    let mut frame = lookup
        .symbol
        .as_deref()
        .map_or_else(|| format!("0x{address:016x}"), compact_runtime_symbol);
    if let Some(filename) = &lookup.filename {
        let path = std::path::Path::new(filename);
        let short_path = path
            .components()
            .rev()
            .take(4)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .map(|component| component.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("\\");
        write!(frame, " ({short_path}").unwrap();
        if let Some(line) = lookup.line {
            write!(frame, ":{line}").unwrap();
        }
        frame.push(')');
    }
    frame
}

fn compact_runtime_symbol(symbol: &str) -> String {
    let mut compact = String::with_capacity(symbol.len());
    let mut generic_depth = 0;
    for character in symbol.chars() {
        match character {
            '<' => generic_depth += 1,
            '>' if generic_depth != 0 => generic_depth -= 1,
            _ if generic_depth == 0 => compact.push(character),
            _ => {}
        }
    }
    compact
}

fn is_runtime_recorder_frame(symbol: &str) -> bool {
    symbol.starts_with("backtrace::")
        || symbol.contains("seismograph::recorder::")
        || symbol.contains("seismograph::recorder::")
        || symbol.contains("performables::arc::")
        || symbol.contains("performables::sync::mutex::")
        || symbol.contains("performables::sync::lock::")
}

fn render_domains(html: &mut String, snapshot: &Snapshot) {
    write!(
        html,
        "<section><h2>{}</h2>",
        concept(
            "Allocation domains",
            "Independent region pools shared by related heaps. Fragmentation and retained slices do not cross domain boundaries."
        )
    )
    .unwrap();
    if snapshot.domains.is_empty() {
        html.push_str("<p class=\"section-note\">Domain telemetry is unavailable in this snapshot.</p></section>");
        return;
    }

    write!(
        html,
        "<p class=\"section-note\">Each domain can grow through multiple 1 GiB regions. Used slices are grouped by their current allocator purpose.</p><table><tr><th>{}</th><th>{}</th><th>{}</th><th>{}</th><th>{}</th><th>{}</th><th>{}</th><th>{}</th></tr>",
        concept("Domain", "Stable process-local identity assigned when the domain is created."),
        concept("Regions", "Region indices currently owned exclusively by this domain."),
        concept("Reserved", "Virtual address space reserved by all regions in the domain."),
        concept("Used / free slices", "64 KiB slices assigned to allocator structures versus available in this domain."),
        concept("Small", "Slices containing small-allocation slab segments."),
        concept("Medium", "Slices backing live or retained medium spans."),
        concept("Bump / other", "Bump chunks followed by allocated slices without a stable classification."),
        concept("Used-slice mix", "Relative composition of used slices: small slabs, medium spans, bump chunks, and unclassified backing."),
    )
    .unwrap();
    for domain in &snapshot.domains {
        let label = if domain.is_default {
            format!("#{} <span class=\"muted\">default</span>", domain.domain_id)
        } else {
            format!("#{}", domain.domain_id)
        };
        let regions = if domain.region_indices.is_empty() {
            "—".to_owned()
        } else {
            domain
                .region_indices
                .iter()
                .map(|index| format!("#{index}"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let classified = domain
            .small_slices
            .saturating_add(domain.medium_slices)
            .saturating_add(domain.bump_slices)
            .saturating_add(domain.unknown_slices);
        let percentage = |value: u64| {
            if classified == 0 {
                0.0
            } else {
                100.0 * value as f64 / classified as f64
            }
        };
        write!(
            html,
            "<tr><td>{}</td><td>{} <span class=\"muted\">({})</span></td><td>{}</td><td>{} / {}</td><td>{}</td><td>{}</td><td>{} / {}</td><td><div class=\"domain-bar\" title=\"small {} · medium {} · bump {} · other {}\"><span class=\"small-bg\" style=\"width:{:.3}%\"></span><span class=\"medium-bg\" style=\"width:{:.3}%\"></span><span class=\"bump-bg\" style=\"width:{:.3}%\"></span><span class=\"unknown-bg\" style=\"width:{:.3}%\"></span></div></td></tr>",
            label,
            regions,
            format_count(domain.region_count),
            format_bytes(domain.reserved_bytes),
            format_count(domain.used_slices),
            format_count(domain.free_slices),
            format_count(domain.small_slices),
            format_count(domain.medium_slices),
            format_count(domain.bump_slices),
            format_count(domain.unknown_slices),
            format_count(domain.small_slices),
            format_count(domain.medium_slices),
            format_count(domain.bump_slices),
            format_count(domain.unknown_slices),
            percentage(domain.small_slices),
            percentage(domain.medium_slices),
            percentage(domain.bump_slices),
            percentage(domain.unknown_slices),
        )
        .unwrap();
    }
    html.push_str("</table></section>");
}

fn render_physical_topology(html: &mut String, snapshot: &Snapshot) {
    write!(
        html,
        "<section id=\"physical-topology\" class=\"topology-kind\"><h2>{}</h2>",
        concept(
            "Virtual regions",
            "Large virtual-address reservations from which rallocator assigns equal-sized slices."
        )
    )
    .unwrap();
    if snapshot.topology.is_empty() {
        write!(
            html,
            "<p class=\"section-note\">Detailed slice topology is unavailable in this snapshot.</p><table><tr><th>{}</th><th>{}</th><th>{}</th><th>{}</th></tr>",
            concept("Region", "A large reserved virtual-address range managed as equal-sized slices."),
            concept("Reserved", "Virtual address space reserved for this region."),
            concept("Used slices", "Slices currently assigned to allocator structures."),
            concept("Free slices", "Slices available for future allocator structures."),
        )
        .unwrap();
        for region in &snapshot.regions {
            write!(
                html,
                "<tr><td>#{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
                region.region_index,
                format_bytes(region.reserved_bytes),
                format_count(region.used_slices),
                format_count(region.free_slices),
            )
            .unwrap();
        }
        html.push_str("</table></section>");
        return;
    }

    html.push_str("<p class=\"section-note\">The selected diagram is one allocator-reserved region. One pixel is one physical slice. Small-slab pixels become brighter as more of their blocks are live when aggregate tracking was enabled.</p><div class=\"topology-controls\"><fieldset class=\"topology-regions\"><legend>Region</legend><label for=\"topology-region-selector\">Visualize</label><select id=\"topology-region-selector\">");
    for (index, region) in snapshot.topology.iter().enumerate() {
        write!(
            html,
            "<option value=\"{}\"{}>Region #{}</option>",
            region.region_index,
            if index == 0 { " selected" } else { "" },
            region.region_index,
        )
        .unwrap();
    }
    html.push_str("</select></fieldset><fieldset class=\"topology-modes\"><legend>Slice colors</legend><label><input type=\"radio\" name=\"topology-mode\" value=\"kind\" checked> Slice type</label><label><input type=\"radio\" name=\"topology-mode\" value=\"owner\"> Heap / owner identity</label></fieldset></div>");
    write!(
        html,
        "<div class=\"legend kind-legend\"><span><i class=\"swatch free-bg\"></i>{}</span><span><i class=\"swatch small-bg\"></i>{}</span><span><i class=\"swatch medium-bg\"></i>{}</span><span><i class=\"swatch bump-bg\"></i>{}</span><span><i class=\"swatch unknown-bg\"></i>{}</span></div><div class=\"legend owner-legend\"><span><i class=\"swatch owner-sample-a\"></i>Heap / owner identities</span><span><i class=\"swatch unknown-bg\"></i>Unowned / transitional</span><span><i class=\"swatch free-bg\"></i>Free</span></div>",
        concept("Free", "A slice not currently assigned to an allocator structure."),
        concept("Small slabs", "A slice containing one or two 32 KiB slab segments for small size-class allocations."),
        concept("Medium span", "One or more contiguous 64 KiB slices backing a medium allocation or retained medium extent."),
        concept("Bump chunk", "A 64 KiB chunk used by a bump heap for sequential allocation."),
        concept("Allocated / transitional", "A used slice whose detailed purpose was not published or changed during snapshot collection."),
    )
    .unwrap();
    for (index, region) in snapshot.topology.iter().enumerate() {
        write!(
            html,
            "<div class=\"topology-region\" data-region=\"{}\"{}>",
            region.region_index,
            if index == 0 { "" } else { " hidden" },
        )
        .unwrap();
        render_region(
            html,
            region,
            snapshot
                .domains
                .iter()
                .find(|domain| domain.region_indices.contains(&region.region_index)),
        );
        html.push_str("</div>");
    }
    html.push_str("</section>");
}

fn render_region(html: &mut String, region: &TopologyRegion, domain: Option<&Domain>) {
    let total_slices = region
        .region_bytes
        .checked_div(region.slice_bytes)
        .unwrap_or(region.used_bitmap.len() as u64 * 64);
    let used_slices = region.used_bitmap.iter().map(|word| u64::from(word.count_ones())).sum::<u64>();
    let width = grid_width(total_slices);
    let height = total_slices.div_ceil(width);
    let details = region
        .slices
        .iter()
        .map(|slice| (slice.slice_index, slice))
        .collect::<HashMap<_, _>>();

    write!(
        html,
        "<h3>Region #{}{} </h3><div class=\"region\">",
        region.region_index,
        domain.map_or_else(String::new, |domain| {
            format!(
                " <span class=\"muted\">· domain #{}{}</span>",
                domain.domain_id,
                if domain.is_default { " default" } else { "" }
            )
        })
    )
    .unwrap();
    write!(
        html,
        "<svg class=\"slice-map\" viewBox=\"0 0 {width} {height}\" role=\"img\" aria-label=\"Region {} slice map\">",
        region.region_index
    )
    .unwrap();
    for slice_index in 0..total_slices {
        if !bitmap_contains(&region.used_bitmap, slice_index) {
            continue;
        }
        let detail = details.get(&(slice_index as u32)).copied();
        let kind = detail.map_or(SliceKind::Unknown, |slice| slice.kind);
        let owner = detail.map_or(0, |slice| slice.owner);
        let utilization = detail.and_then(slice_block_utilization);
        let utilization_style = utilization.map_or_else(String::new, |(live, usable)| {
            let ratio = if usable == 0 { 0.0 } else { live as f64 / usable as f64 };
            format!(" style=\"fill-opacity:{:.3}\"", 0.82f64.mul_add(ratio, 0.18).clamp(0.18, 1.0))
        });
        let utilization_label = utilization.map_or_else(String::new, |(live, usable)| {
            format!(
                " · {} / {} live blocks ({:.1}%)",
                live,
                usable,
                if usable == 0 { 0.0 } else { 100.0 * live as f64 / usable as f64 }
            )
        });
        let x = slice_index % width;
        let y = slice_index / width;
        write!(
            html,
            "<rect class=\"{}\" data-owner=\"{}\" style=\"--owner-color:{};{}\" x=\"{}\" y=\"{}\" width=\"1\" height=\"1\"><title>slice {} · {} · owner {}{} · 0x{:016x}</title></rect>",
            slice_kind_class(kind),
            owner,
            owner_color(owner),
            utilization_style
                .strip_prefix(" style=\"")
                .and_then(|style| style.strip_suffix('"'))
                .unwrap_or(""),
            x,
            y,
            slice_index,
            slice_kind_label(kind),
            format_owner(owner),
            utilization_label,
            region
                .base_address
                .saturating_add(slice_index.saturating_mul(region.slice_bytes)),
        )
        .unwrap();
    }
    html.push_str("</svg><div><table class=\"compact\"><tr><th>Property</th><th>Value</th></tr>");
    write!(
        html,
        "<tr><td>{}</td><td><code>0x{:016x}–0x{:016x}</code></td></tr><tr><td>{}</td><td>{}</td></tr><tr><td>{}</td><td>{}</td></tr><tr><td>{}</td><td>{}</td></tr><tr><td>{}</td><td>{} / {}</td></tr><tr><td>{}</td><td>{:.2}%</td></tr>",
        concept("Address range", "The inclusive start and exclusive end of the region's reserved virtual addresses."),
        region.base_address,
        region.base_address.saturating_add(region.region_bytes),
        concept("Reserved", "Virtual address space reserved for this region."),
        format_bytes(region.region_bytes),
        concept("Slice size", "The allocator's physical allocation unit within a region; normally 64 KiB."),
        format_bytes(region.slice_bytes),
        concept("Slices", "Total number of equal-sized physical slices in this region."),
        format_count(total_slices),
        concept("Used / free", "Slices assigned to allocator structures versus slices available for reuse."),
        format_count(used_slices),
        format_count(total_slices.saturating_sub(used_slices)),
        concept("Physical utilization", "Used slices divided by all slices in this reserved region."),
        if total_slices == 0 {
            0.0
        } else {
            100.0 * used_slices as f64 / total_slices as f64
        },
    )
    .unwrap();
    html.push_str("</table></div></div>");

    write!(
        html,
        "<details><summary>{}</summary><h3>{}</h3><table class=\"compact\"><tr><th>{}</th><th>{}</th><th>{}</th><th>Bytes</th></tr>",
        concept(
            "Slice runs and classified slice details",
            "Expanded physical records for contiguous used/free ranges and every used slice."
        ),
        concept(
            "Contiguous physical runs",
            "Adjacent slices with the same used or free state grouped into ranges."
        ),
        concept("State", "Whether the slices are assigned or available."),
        concept("First slice", "Index of the first slice in the contiguous run."),
        concept("Slice count", "Number of consecutive slices in the run."),
    )
    .unwrap();
    for (used, first, count) in slice_runs(&region.used_bitmap, total_slices) {
        write!(
            html,
            "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
            if used { "Used" } else { "Free" },
            format_count(first),
            format_count(count),
            format_bytes(count.saturating_mul(region.slice_bytes)),
        )
        .unwrap();
    }
    write!(
        html,
        "</table><h3>{}</h3><table class=\"compact\"><tr><th>{}</th><th>Address</th><th>{}</th><th>{}</th><th>{}</th><th>{}</th><th>{}</th></tr>",
        concept("Used slices", "Physical slices currently assigned to allocator structures."),
        concept("Slice", "Zero-based physical slice index within the region."),
        concept("Kind", "The allocator structure currently represented by the slice."),
        concept("Span", "Number of contiguous slices in a structure beginning at this slice."),
        concept("Owner", "Opaque identity of the heap, bump state, or owner associated with this structure."),
        concept("Requested / usable", "Application-requested bytes and allocator-provided capacity for a live medium span."),
        concept("Segments", "The 32 KiB small-allocation slab halves initialized inside this 64 KiB slice."),
    )
    .unwrap();
    for slice in &region.slices {
        let segments = slice
            .segments
            .iter()
            .map(|segment| {
                if segment.utilization_tracked {
                    format!(
                        "{}: class #{}{} · {} / {} live ({:.1}%)",
                        segment.segment_index,
                        segment.class_index,
                        if segment.context { " context" } else { "" },
                        segment.live_blocks,
                        segment.usable_blocks,
                        if segment.usable_blocks == 0 {
                            0.0
                        } else {
                            100.0 * f64::from(segment.live_blocks) / f64::from(segment.usable_blocks)
                        }
                    )
                } else {
                    format!(
                        "{}: class #{}{} · utilization unavailable",
                        segment.segment_index,
                        segment.class_index,
                        if segment.context { " context" } else { "" }
                    )
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        write!(
            html,
            "<tr><td>{}</td><td><code>0x{:016x}</code></td><td>{}</td><td>{}</td><td>{}</td><td>{} / {}</td><td>{}</td></tr>",
            slice.slice_index,
            region
                .base_address
                .saturating_add(u64::from(slice.slice_index).saturating_mul(region.slice_bytes)),
            slice_kind_label(slice.kind),
            slice.span_slices,
            format_owner(slice.owner),
            format_bytes(slice.requested_bytes),
            format_bytes(slice.usable_bytes),
            if segments.is_empty() { "—" } else { &segments },
        )
        .unwrap();
    }
    html.push_str("</table></details>");
}

fn render_allocator_structures(html: &mut String, snapshot: &Snapshot) {
    let mut segments = BTreeMap::<(u32, bool, u64, u64), u64>::new();
    let mut medium = Vec::new();
    let mut bumps = BTreeMap::<(u64, u64), u64>::new();
    let mut owners = BTreeMap::<u64, [u64; 3]>::new();
    let block_sizes = snapshot
        .size_classes
        .iter()
        .map(|class| (class.class_index, class.block_bytes))
        .collect::<HashMap<_, _>>();

    for region in &snapshot.topology {
        for slice in &region.slices {
            for segment in &slice.segments {
                let segment_bytes = region.slice_bytes / 2;
                *segments
                    .entry((segment.class_index, segment.context, slice.owner, segment_bytes))
                    .or_default() += 1;
                if slice.owner != 0 {
                    owners.entry(slice.owner).or_default()[0] += 1;
                }
            }
            if slice.kind == SliceKind::Medium {
                medium.push((region, slice));
                if slice.owner != 0 {
                    owners.entry(slice.owner).or_default()[1] += 1;
                }
            } else if slice.kind == SliceKind::Bump {
                *bumps.entry((slice.owner, region.slice_bytes)).or_default() += 1;
                if slice.owner != 0 {
                    owners.entry(slice.owner).or_default()[2] += 1;
                }
            }
        }
    }

    write!(
        html,
        "<section><details><summary>{}</summary><h3>{}</h3><table class=\"compact\"><tr><th>{}</th><th>{}</th><th>{}</th><th>{}</th><th>{}</th><th>{}</th></tr>",
        concept("Segments, spans, bumps, and owners", "The allocator structures layered on top of virtual-address region slices."),
        concept("Small-allocation slab segments", "Slab halves divided into fixed-size blocks for one size class."),
        concept("Class", "The size-class index selecting a fixed block size."),
        concept("Block size", "Usable bytes provided by every allocation block housed in this slab segment."),
        concept("Role", "General slabs serve ordinary allocations; context slabs support allocator context metadata and special paths."),
        concept("Owner", "Opaque identity associated with the heap or remote owner managing these segments."),
        concept("Segments", "Count of slab halves in this group."),
        concept("Backing bytes", "Total virtual-region backing capacity occupied by these slab segments."),
    )
    .unwrap();
    for ((class_index, context, owner, segment_bytes), count) in &segments {
        write!(
            html,
            "<tr><td>#{class_index}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
            block_sizes
                .get(class_index)
                .copied()
                .map_or_else(|| "Unknown".to_owned(), format_bytes),
            if *context { "Context" } else { "General" },
            format_owner(*owner),
            format_count(*count),
            format_bytes(count.saturating_mul(*segment_bytes)),
        )
        .unwrap();
    }
    if segments.is_empty() {
        html.push_str("<tr><td colspan=\"6\" class=\"muted\">No classified small segments.</td></tr>");
    }

    write!(
        html,
        "</table><h3>{}</h3><table class=\"compact\"><tr><th>{}</th><th>{}</th><th>{}</th><th>{}</th><th>{}</th><th>{}</th></tr>",
        concept(
            "Medium spans",
            "Contiguous region slices used for allocations too large for small slabs but below direct-mapping thresholds."
        ),
        concept("Region / slice", "Virtual-address region and starting slice of the span."),
        concept(
            "State",
            "Whether the span backs a live allocation or is retained in a medium cache/free list."
        ),
        concept("Span", "Number of contiguous region slices in the medium extent."),
        concept("Requested", "Bytes requested by the live medium allocation."),
        concept("Usable", "Capacity provided by the complete slice span."),
        concept(
            "Owner",
            "Opaque identity of the heap currently owning the live allocation; absent for retained free spans."
        ),
    )
    .unwrap();
    for (region, slice) in &medium {
        write!(
            html,
            "<tr><td>#{} / {}</td><td>{}</td><td>{} slices</td><td>{}</td><td>{}</td><td>{}</td></tr>",
            region.region_index,
            slice.slice_index,
            if slice.owner == 0 {
                "Retained free span"
            } else {
                "Live allocation"
            },
            slice.span_slices,
            format_bytes(slice.requested_bytes),
            format_bytes(slice.usable_bytes),
            format_owner(slice.owner),
        )
        .unwrap();
    }
    if medium.is_empty() {
        html.push_str("<tr><td colspan=\"6\" class=\"muted\">No medium spans.</td></tr>");
    }

    write!(
        html,
        "</table><h3>{}</h3><table class=\"compact\"><tr><th>{}</th><th>{}</th><th>{}</th></tr>",
        concept(
            "Bump backing",
            "Region slices retained by bump heaps for fast sequential allocation and bulk reuse."
        ),
        concept(
            "Bump state",
            "Opaque identity of one bump heap state shared by its chunks and escaped allocations."
        ),
        concept("Chunks", "Number of region slices assigned as chunks to this bump state."),
        concept("Backing bytes", "Total slice capacity retained by the bump state."),
    )
    .unwrap();
    for ((owner, slice_bytes), chunks) in &bumps {
        write!(
            html,
            "<tr><td>{}</td><td>{}</td><td>{}</td></tr>",
            format_owner(*owner),
            format_count(*chunks),
            format_bytes(chunks.saturating_mul(*slice_bytes)),
        )
        .unwrap();
    }
    if bumps.is_empty() {
        html.push_str("<tr><td colspan=\"3\" class=\"muted\">No bump chunks.</td></tr>");
    }

    write!(
        html,
        "</table><h3>{}</h3><table class=\"compact\"><tr><th>{}</th><th>{}</th><th>{}</th><th>{}</th></tr>",
        concept(
            "Known allocator owners",
            "Opaque owner identities observed in published physical topology records."
        ),
        concept(
            "Owner identity",
            "A snapshot-local pointer identity used only to correlate structures belonging to the same owner."
        ),
        concept(
            "Small segments",
            "Small-allocation slab segments associated with this owner identity."
        ),
        concept("Medium spans", "Live medium span starts associated with this owner identity."),
        concept("Bump chunks", "Bump chunks associated with this owner identity."),
    )
    .unwrap();
    for (owner, counts) in &owners {
        write!(
            html,
            "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
            format_owner(*owner),
            format_count(counts[0]),
            format_count(counts[1]),
            format_count(counts[2]),
        )
        .unwrap();
    }
    if owners.is_empty() {
        html.push_str("<tr><td colspan=\"4\" class=\"muted\">No owner identities were published.</td></tr>");
    }
    html.push_str("</table></details></section>");
}

fn render_size_classes(html: &mut String, snapshot: &Snapshot) {
    write!(
        html,
        "<section><details><summary>{}</summary><p class=\"section-note\">Payload efficiency is requested bytes divided by usable bytes in live blocks. It is not slab or region occupancy.</p><table><tr><th>{}</th><th>{}</th><th>{}</th><th>{}</th><th>{}</th><th>{}</th></tr>",
        concept("General slabs and size classes", "Aggregated live-allocation information for fixed-size small-allocation classes."),
        concept("Class", "The allocator's index for a fixed block size."),
        concept("Block", "Usable bytes provided by one block in this size class."),
        concept("Live allocations", "Estimated number of currently live blocks in this class."),
        concept("Requested", "Estimated sum of application-requested bytes in live blocks."),
        concept("Usable", "Estimated sum of full block capacity for live allocations."),
        concept("Payload efficiency", "Requested bytes divided by usable bytes in live blocks; this does not measure slab fullness."),
    )
    .unwrap();
    for class in &snapshot.size_classes {
        let efficiency = if class.usable_bytes.value == 0 {
            0.0
        } else {
            100.0 * class.requested_bytes.value as f64 / class.usable_bytes.value as f64
        };
        write!(
            html,
            "<tr><td>#{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td><div class=\"bar\"><span style=\"width:{:.1}%\"></span></div>{:.1}%</td></tr>",
            class.class_index,
            format_bytes(class.block_bytes),
            format_estimate(class.live_allocations),
            format_estimate_bytes(class.requested_bytes),
            format_estimate_bytes(class.usable_bytes),
            efficiency.clamp(0.0, 100.0),
            efficiency,
        )
        .unwrap();
    }
    html.push_str("</table></details></section>");
}

fn render_allocation_histograms(html: &mut String, snapshot: &Snapshot) {
    write!(
        html,
        "<section><h2>{}</h2><p class=\"section-note\">Buckets are powers of two: a 4 KiB bucket covers sizes from 2,049 through 4,096 bytes.</p>",
        concept(
            "Allocation size histograms",
            "All allocation operations recorded by aggregate telemetry and allocations currently live at capture time."
        )
    )
    .unwrap();
    render_histogram(
        html,
        &snapshot.histograms.allocated,
        &snapshot.histograms.live,
        "process-allocation-sizes",
        "All allocations",
        "Currently live",
    );
    html.push_str("</section>");
}

fn render_thread_histograms(html: &mut String, callers: &Callers) {
    let names = thread_name_map(callers);
    html.push_str("<details><summary>Allocation sizes by thread</summary>");
    if callers.threads.is_empty() {
        html.push_str("<p class=\"muted\">No caller thread logs.</p></details>");
        return;
    }
    for thread in &callers.threads {
        let thread_id = callers
            .events
            .iter()
            .find(|event| event.thread_log_id == thread.thread_log_id)
            .map(|event| event.event_thread_id);
        let label = thread_id.map_or_else(
            || format!("Thread log #{}", thread.thread_log_id),
            |thread_id| thread_label(thread_id, &names),
        );
        write!(
            html,
            "<details><summary>{} · log #{} · {} events · {} lost</summary>",
            escape_html(&label),
            thread.thread_log_id,
            format_count(thread.total_events),
            format_count(thread.lost_events),
        )
        .unwrap();
        render_histogram(
            html,
            &thread.allocated_histogram,
            &thread.live_histogram,
            &format!("thread-log-{}-allocation-sizes", thread.thread_log_id),
            "Allocated",
            "Live",
        );
        html.push_str("</details>");
    }
    html.push_str("</details>");
}

fn render_histogram(html: &mut String, allocated: &[u64], live: &[u64], id: &str, allocated_label: &str, live_label: &str) {
    let maximum = live.iter().copied().max().unwrap_or(0);
    write!(
        html,
        "<fieldset class=\"histogram-controls\"><legend>Show</legend><label><input type=\"radio\" name=\"{}-mode\" value=\"live\" checked> {}</label><label><input type=\"radio\" name=\"{}-mode\" value=\"allocated\"> {}</label></fieldset><div class=\"histogram histogram-live\" data-histogram=\"{}\">",
        escape_html(id),
        escape_html(live_label),
        escape_html(id),
        escape_html(allocated_label),
        escape_html(id),
    )
    .unwrap();
    for bucket in 0..allocated.len().max(live.len()) {
        let allocated_count = allocated.get(bucket).copied().unwrap_or(0);
        let live_count = live.get(bucket).copied().unwrap_or(0);
        if allocated_count == 0 && live_count == 0 {
            continue;
        }
        let allocated_height = histogram_height(allocated_count, allocated.iter().copied().max().unwrap_or(0));
        let live_height = histogram_height(live_count, maximum);
        write!(
            html,
            "<div class=\"histogram-bucket\" title=\"{}: {} · {}: {}\"><div class=\"histogram-plot\"><i class=\"allocated\" style=\"height:{allocated_height}%\"></i><i class=\"live\" style=\"height:{live_height}%\"></i></div><span class=\"histogram-label\">{}</span></div>",
            escape_html(allocated_label),
            format_count(allocated_count),
            escape_html(live_label),
            format_count(live_count),
            histogram_label(bucket),
        )
        .unwrap();
    }
    html.push_str("</div>");
}

fn histogram_height(count: u64, maximum: u64) -> f64 {
    if count == 0 || maximum == 0 {
        0.0
    } else {
        100.0 * (count as f64).ln_1p() / (maximum as f64).ln_1p()
    }
}

fn histogram_label(bucket: usize) -> String {
    if bucket == 0 {
        return "0 B".to_owned();
    }
    let upper = 1_u64.checked_shl((bucket - 1) as u32).unwrap_or(u64::MAX);
    format_bytes(upper)
}

#[derive(Default)]
struct Hotspot {
    count: u64,
    bytes: u64,
}

#[derive(Clone, Copy)]
enum SupportedEventKind {
    Allocated,
    Deallocated,
}

#[cfg_attr(coverage_nightly, coverage(off))]
fn supported_events(events: &[Event]) -> impl Iterator<Item = (&Event, SupportedEventKind)> {
    events.iter().filter_map(|event| {
        let kind = match event.kind {
            EventKind::Allocated => SupportedEventKind::Allocated,
            EventKind::Deallocated => SupportedEventKind::Deallocated,
            _ => return None,
        };
        Some((event, kind))
    })
}

fn render_hotspots(html: &mut String, callers: &Callers, addresses: &[AddressLookup]) {
    let mut live = HashMap::<(u64, u64), &Event>::new();
    let mut cross_thread = HashMap::<(Vec<u64>, Vec<u64>), Hotspot>::new();
    let mut escaped_bump = HashMap::<(Vec<u64>, Vec<u64>), Hotspot>::new();
    let mut thread_flows = BTreeMap::<(u64, u64), (u64, u64)>::new();
    for (event, kind) in supported_events(&callers.events) {
        match kind {
            SupportedEventKind::Allocated => {
                live.insert((event.thread_log_id, event.allocation_id), event);
            }
            SupportedEventKind::Deallocated => {
                let Some(allocation) = live.remove(&(event.thread_log_id, event.allocation_id)) else {
                    continue;
                };
                let flow = thread_flows.entry((allocation.event_thread_id, event.event_thread_id)).or_default();
                flow.0 += 1;
                flow.1 += allocation.size;
                if allocation.event_thread_id != event.event_thread_id {
                    record_hotspot(&mut cross_thread, allocation, event);
                }
                if event.heap_kind == HeapKind::Bump && event.freed_after_heap_release {
                    record_hotspot(&mut escaped_bump, allocation, event);
                }
            }
        }
    }

    html.push_str("<h3>Cross-thread and escaped-lifetime hotspots</h3>");
    render_thread_flow_diagram(html, callers, &thread_flows);
    render_hotspot_group(html, "Allocated and freed on different threads", &cross_thread, addresses);
    render_hotspot_group(html, "Freed after a bump heap handle was released", &escaped_bump, addresses);
}

fn record_hotspot(hotspots: &mut HashMap<(Vec<u64>, Vec<u64>), Hotspot>, allocation: &Event, deallocation: &Event) {
    let key = (allocation.call_stack.clone(), deallocation.call_stack.clone());
    let hotspot = hotspots.entry(key).or_default();
    hotspot.count += 1;
    hotspot.bytes += allocation.size;
}

fn render_hotspot_group(html: &mut String, title: &str, hotspots: &HashMap<(Vec<u64>, Vec<u64>), Hotspot>, addresses: &[AddressLookup]) {
    let lookups = addresses.iter().map(|lookup| (lookup.address, lookup)).collect::<HashMap<_, _>>();
    let mut hotspots = hotspots.iter().collect::<Vec<_>>();
    hotspots.sort_unstable_by(|(_, left), (_, right)| right.bytes.cmp(&left.bytes).then_with(|| right.count.cmp(&left.count)));
    let empty = hotspots.is_empty();
    let total = hotspots.len();
    write!(
        html,
        "<details><summary>{} (showing {} of {})</summary><ol>",
        escape_html(title),
        total.min(16),
        total
    )
    .unwrap();
    for ((allocation_stack, deallocation_stack), hotspot) in hotspots.into_iter().take(16) {
        write!(
            html,
            "<li><strong>{} across {} allocations</strong><div class=\"hotspot-stacks\"><div><b>Allocation</b>",
            format_bytes(hotspot.bytes),
            format_count(hotspot.count),
        )
        .unwrap();
        render_stack(html, allocation_stack, &lookups);
        html.push_str("</div><div><b>Deallocation</b>");
        render_stack(html, deallocation_stack, &lookups);
        html.push_str("</div></div></li>");
    }
    if empty {
        html.push_str("<li class=\"muted\">No matching retained events.</li>");
    }
    html.push_str("</ol></details>");
}

fn render_stack(html: &mut String, stack: &[u64], lookups: &HashMap<u64, &AddressLookup>) {
    write!(
        html,
        "<details class=\"stack-details\"><summary>Stack trace · {} frames</summary><div class=\"stack\">",
        stack.len()
    )
    .unwrap();
    if stack.is_empty() {
        html.push_str("<div>Stack capture disabled</div>");
    } else {
        for address in stack {
            let frame = format_frame(*address, lookups.get(address).copied());
            write!(html, "<div>{}</div>", escape_html(&frame)).unwrap();
        }
    }
    html.push_str("</div></details>");
}

fn render_thread_flow_diagram(html: &mut String, callers: &Callers, flows: &BTreeMap<(u64, u64), (u64, u64)>) {
    html.push_str("<details><summary>Allocation → free thread flow</summary>");
    if flows.is_empty() {
        html.push_str("<p class=\"muted\">No retained frees.</p></details>");
        return;
    }
    let names = thread_name_map(callers);
    let threads = flows
        .keys()
        .flat_map(|(source, destination)| [*source, *destination])
        .collect::<std::collections::BTreeSet<_>>();
    let rows = threads
        .iter()
        .enumerate()
        .map(|(index, thread)| (*thread, (index as f64).mul_add(68.0, 68.0)))
        .collect::<HashMap<_, _>>();
    let height = 98 + threads.len() * 68;
    let maximum = flows.values().map(|(_, bytes)| *bytes).max().unwrap_or(1);
    let source_totals = flow_totals(flows, true);
    let destination_totals = flow_totals(flows, false);
    write!(
        html,
        "<div class=\"thread-flow-scroll\"><svg class=\"thread-flow\" viewBox=\"0 0 1200 {height}\" role=\"img\" aria-label=\"Allocation and free thread flow\"><defs><marker id=\"thread-flow-arrow-cross\" markerWidth=\"5\" markerHeight=\"5\" refX=\"4.5\" refY=\"2.5\" orient=\"auto\"><path d=\"M0,0 L5,2.5 L0,5 z\"></path></marker><marker id=\"thread-flow-arrow-local\" markerWidth=\"5\" markerHeight=\"5\" refX=\"4.5\" refY=\"2.5\" orient=\"auto\"><path d=\"M0,0 L5,2.5 L0,5 z\"></path></marker></defs><text class=\"flow-heading\" x=\"18\" y=\"24\">Allocation thread</text><text class=\"flow-heading\" x=\"1182\" y=\"24\" text-anchor=\"end\">Free thread</text><g class=\"flow-legend\"><line class=\"thread-flow-link local\" x1=\"430\" y1=\"22\" x2=\"470\" y2=\"22\"></line><text x=\"480\" y=\"26\">same thread</text><line class=\"thread-flow-link cross\" x1=\"690\" y1=\"22\" x2=\"730\" y2=\"22\"></line><text x=\"740\" y=\"26\">different thread</text></g>"
    )
    .unwrap();
    for thread in &threads {
        let y = rows[thread];
        let (source_count, source_bytes) = source_totals.get(thread).copied().unwrap_or_default();
        let (destination_count, destination_bytes) = destination_totals.get(thread).copied().unwrap_or_default();
        write!(
            html,
            "<line class=\"thread-flow-row\" x1=\"18\" y1=\"{y}\" x2=\"1182\" y2=\"{y}\"></line><g class=\"thread-flow-endpoint source\" data-side=\"source\" data-thread=\"{}\" tabindex=\"0\" role=\"button\"><rect class=\"thread-flow-node\" x=\"18\" y=\"{}\" width=\"312\" height=\"48\" rx=\"7\"></rect><text class=\"flow-thread-name\" x=\"32\" y=\"{}\">{}</text><text class=\"flow-thread-total\" x=\"32\" y=\"{}\">{} allocations · {}</text></g><g class=\"thread-flow-endpoint destination\" data-side=\"destination\" data-thread=\"{}\" tabindex=\"0\" role=\"button\"><rect class=\"thread-flow-node\" x=\"870\" y=\"{}\" width=\"312\" height=\"48\" rx=\"7\"></rect><text class=\"flow-thread-name\" x=\"1168\" y=\"{}\" text-anchor=\"end\">{}</text><text class=\"flow-thread-total\" x=\"1168\" y=\"{}\" text-anchor=\"end\">{} frees · {}</text></g>",
            thread,
            y - 24.0,
            y - 4.0,
            escape_html(&thread_label(*thread, &names)),
            y + 14.0,
            format_count(source_count),
            format_bytes(source_bytes),
            thread,
            y - 24.0,
            y - 4.0,
            escape_html(&thread_label(*thread, &names)),
            y + 14.0,
            format_count(destination_count),
            format_bytes(destination_bytes),
        )
        .unwrap();
    }
    for (&(source, destination), &(count, bytes)) in flows {
        let source_y = rows[&source];
        let destination_y = rows[&destination];
        let width = 1.5 + 8.5 * (bytes as f64).ln_1p() / (maximum as f64).ln_1p();
        let class = if source == destination { "local" } else { "cross" };
        let label_y = f64::midpoint(source_y, destination_y) - 7.0;
        write!(
            html,
            "<path class=\"thread-flow-link {class}\" data-source=\"{source}\" data-destination=\"{destination}\" d=\"M330 {source_y} C500 {source_y},700 {destination_y},870 {destination_y}\" style=\"stroke-width:{width}\"><title>{} → {} · {} · {} frees</title></path><text class=\"thread-flow-label {class}\" data-source=\"{source}\" data-destination=\"{destination}\" x=\"600\" y=\"{label_y}\" text-anchor=\"middle\">{} · {}</text>",
            escape_html(&thread_label(source, &names)),
            escape_html(&thread_label(destination, &names)),
            format_bytes(bytes),
            format_count(count),
            format_bytes(bytes),
            format_count(count),
        )
        .unwrap();
    }
    html.push_str("</svg></div></details>");
}

fn flow_totals(flows: &BTreeMap<(u64, u64), (u64, u64)>, by_source: bool) -> HashMap<u64, (u64, u64)> {
    let mut totals = HashMap::new();
    for (&(source, destination), &(count, bytes)) in flows {
        let thread = if by_source { source } else { destination };
        let total = totals.entry(thread).or_insert((0_u64, 0_u64));
        total.0 += count;
        total.1 += bytes;
    }
    totals
}

fn thread_name_map(callers: &Callers) -> HashMap<u64, &str> {
    callers
        .thread_names
        .iter()
        .map(|thread| (thread.thread_id, thread.name.as_str()))
        .collect()
}

fn thread_label(thread_id: u64, names: &HashMap<u64, &str>) -> String {
    names
        .get(&thread_id)
        .map_or_else(|| format!("Thread #{thread_id}"), |name| format!("{name} · #{thread_id}"))
}

fn owner_color(owner: u64) -> String {
    if owner == 0 {
        return "#7d879e".to_owned();
    }
    let hue = owner.wrapping_mul(11_400_714_819_323_198_485) % 360;
    format!("hsl({hue} 68% 58%)")
}

fn bitmap_contains(bitmap: &[u64], slice_index: u64) -> bool {
    bitmap
        .get((slice_index / 64) as usize)
        .is_some_and(|word| word & (1_u64 << (slice_index % 64)) != 0)
}

fn slice_runs(bitmap: &[u64], total_slices: u64) -> Vec<(bool, u64, u64)> {
    if total_slices == 0 {
        return Vec::new();
    }
    let mut runs = Vec::new();
    let mut first = 0;
    let mut used = bitmap_contains(bitmap, 0);
    for slice in 1..total_slices {
        let next_used = bitmap_contains(bitmap, slice);
        if next_used != used {
            runs.push((used, first, slice - first));
            first = slice;
            used = next_used;
        }
    }
    runs.push((used, first, total_slices - first));
    runs
}

fn grid_width(total_slices: u64) -> u64 {
    if total_slices >= 16_384 {
        return 128;
    }
    let mut width = 1_u64;
    while width.saturating_mul(width) < total_slices {
        width += 1;
    }
    width
}

fn slice_kind_class(kind: SliceKind) -> &'static str {
    match kind {
        SliceKind::Unknown => "unknown",
        SliceKind::Small => "small",
        SliceKind::Medium => "medium",
        SliceKind::MediumContinuation => "medium-continuation",
        SliceKind::Bump => "bump",
    }
}

fn slice_kind_label(kind: SliceKind) -> &'static str {
    match kind {
        SliceKind::Unknown => "Allocated / transitional",
        SliceKind::Small => "Small slab slice",
        SliceKind::Medium => "Medium span start",
        SliceKind::MediumContinuation => "Medium span continuation",
        SliceKind::Bump => "Bump chunk",
    }
}

fn slice_block_utilization(slice: &Slice) -> Option<(u64, u64)> {
    if slice.segments.is_empty() || slice.segments.iter().any(|segment| !segment.utilization_tracked) {
        return None;
    }
    Some(slice.segments.iter().fold((0, 0), |(live, usable), segment| {
        (live + u64::from(segment.live_blocks), usable + u64::from(segment.usable_blocks))
    }))
}

fn format_owner(owner: u64) -> String {
    if owner == 0 {
        "—".to_owned()
    } else {
        format!("<code>0x{owner:016x}</code>")
    }
}

fn concept(label: &str, explanation: &str) -> String {
    format!("{}{}", escape_html(label), info(explanation))
}

fn info(explanation: &str) -> String {
    let explanation = escape_html(explanation);
    format!(
        "<span class=\"info\" tabindex=\"0\" aria-label=\"Information: {explanation}\">i<span class=\"tip\" role=\"tooltip\">{explanation}</span></span>"
    )
}

#[derive(Default)]
struct StackTotal {
    stack: Arc<[u64]>,
    allocations: u64,
    allocated_bytes: u64,
    deallocations: u64,
    live_allocations: u64,
    live_bytes: u64,
}

fn render_callers(html: &mut String, callers: &Callers, addresses: &[AddressLookup]) {
    let mut totals = Vec::<StackTotal>::new();
    let mut indices = HashMap::<Arc<[u64]>, usize>::new();
    let mut live = HashMap::<(u64, u64), (usize, u64)>::new();
    let address_lookups = addresses.iter().map(|lookup| (lookup.address, lookup)).collect::<HashMap<_, _>>();
    for (event, kind) in supported_events(&callers.events) {
        match kind {
            SupportedEventKind::Allocated => {
                let index = if let Some(&index) = indices.get(event.call_stack.as_slice()) {
                    index
                } else {
                    let stack = Arc::<[u64]>::from(event.call_stack.as_slice());
                    let index = totals.len();
                    totals.push(StackTotal {
                        stack: Arc::clone(&stack),
                        ..StackTotal::default()
                    });
                    indices.insert(stack, index);
                    index
                };
                totals[index].allocations += 1;
                totals[index].allocated_bytes += event.size;
                live.insert((event.thread_log_id, event.allocation_id), (index, event.size));
            }
            SupportedEventKind::Deallocated => {
                if let Some((index, _)) = live.remove(&(event.thread_log_id, event.allocation_id)) {
                    totals[index].deallocations += 1;
                }
            }
        }
    }
    for (_, (index, size)) in live {
        totals[index].live_allocations += 1;
        totals[index].live_bytes += size;
    }
    totals.retain(|total| total.live_allocations != 0);
    totals.sort_unstable_by(|left, right| {
        right
            .live_bytes
            .cmp(&left.live_bytes)
            .then_with(|| right.allocated_bytes.cmp(&left.allocated_bytes))
    });

    write!(
        html,
        "<details><summary>{} (showing {} of {})</summary><p class=\"muted\">Session {} · {} events · {} lost · {} thread logs{}</p><ol>",
        if callers.lost_events == 0 {
            "General live-allocation hotspots"
        } else {
            "Uncertain retained-allocation candidates"
        },
        totals.len().min(8),
        totals.len(),
        callers.session_id,
        format_count(callers.total_events),
        format_count(callers.lost_events),
        callers.threads.len(),
        if callers.lost_events == 0 {
            ""
        } else {
            " · lost events can hide matching deallocations"
        }
    )
    .unwrap();
    for total in totals.iter().take(8) {
        write!(
            html,
            "<li><strong>{} {} in {} allocations</strong> <span class=\"muted\">({} allocated in {}, {} freed)</span>",
            format_bytes(total.live_bytes),
            if callers.lost_events == 0 { "live" } else { "unmatched" },
            format_count(total.live_allocations),
            format_bytes(total.allocated_bytes),
            format_count(total.allocations),
            format_count(total.deallocations),
        )
        .unwrap();
        render_stack(html, &total.stack, &address_lookups);
        html.push_str("</li>");
    }
    if totals.is_empty() {
        html.push_str("<li class=\"muted\">No retained allocation stacks.</li>");
    }
    html.push_str("</ol></details>");
}

fn format_frame(address: u64, lookup: Option<&AddressLookup>) -> String {
    let Some(lookup) = lookup else {
        return format!("0x{address:016x}");
    };
    let mut frame = lookup.symbol.clone().unwrap_or_else(|| format!("0x{address:016x}"));
    if let Some(filename) = &lookup.filename {
        write!(frame, " ({filename}").unwrap();
        if let Some(line) = lookup.line {
            write!(frame, ":{line}").unwrap();
            if let Some(column) = lookup.column {
                write!(frame, ":{column}").unwrap();
            }
        }
        frame.push(')');
    }
    if lookup.symbol.is_some() {
        write!(frame, " [0x{address:016x}]").unwrap();
    }
    frame
}

fn format_estimate(value: Estimate) -> String {
    if value.lower_bound == value.upper_bound {
        format_count(value.value)
    } else {
        format!(
            "{} ({}–{})",
            format_count(value.value),
            format_count(value.lower_bound),
            format_count(value.upper_bound)
        )
    }
}

fn format_estimate_bytes(value: Estimate) -> String {
    if value.lower_bound == value.upper_bound {
        format_bytes(value.value)
    } else {
        format!(
            "{} ({}–{})",
            format_bytes(value.value),
            format_bytes(value.lower_bound),
            format_bytes(value.upper_bound)
        )
    }
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.2} {}", UNITS[unit])
    }
}

fn format_count(value: u64) -> String {
    let text = value.to_string();
    let mut formatted = String::with_capacity(text.len() + text.len() / 3);
    for (index, character) in text.chars().enumerate() {
        if index != 0 && (text.len() - index).is_multiple_of(3) {
            formatted.push(',');
        }
        formatted.push(character);
    }
    formatted
}

fn escape_html(value: &str) -> Cow<'_, str> {
    let Some(first_escape) = value.find(['&', '<', '>', '"']) else {
        return Cow::Borrowed(value);
    };
    let mut escaped = String::with_capacity(value.len() + 16);
    escaped.push_str(&value[..first_escape]);
    for character in value[first_escape..].chars() {
        escaped.push_str(match character {
            '&' => "&amp;",
            '<' => "&lt;",
            '>' => "&gt;",
            '"' => "&quot;",
            _ => {
                escaped.push(character);
                continue;
            }
        });
    }
    Cow::Owned(escaped)
}

#[cfg(test)]
mod tests {
    use seismograph::recorder::event::{Event, EventClock, EventKind as RuntimeEventKind, EventPayload, EventTimestamp, Events};
    use seismograph::recorder::thread::ThreadLog;
    use seismograph_rallocator::callers::AddressLookupFields;
    use seismograph_rallocator::snapshot::Version;

    use super::*;

    fn runtime_event(thread: u64, sequence: u64, kind: RuntimeEventKind, object: u64, stack: &[u64]) -> Event {
        Event {
            thread_id: seismograph::recorder::thread::ThreadId::new(thread),
            sequence: seismograph::recorder::event::EventSequence::new(sequence),
            timestamp: EventTimestamp::from_ticks(sequence),
            kind,
            payload: EventPayload::Object(seismograph::recorder::event::ObjectId::new(object)),
            call_stack: stack.iter().copied().map(seismograph::recorder::event::Address::new).collect(),
        }
    }

    #[test]
    fn runtime_render_helpers_cover_empty_zero_and_escaped_content() {
        let metric = RuntimeMetric {
            id: "metric".into(),
            label: "<metric>".into(),
            value: "0".into(),
            magnitude: 0,
            detail: "none & none".into(),
            threads: BTreeMap::new(),
            stacks: Vec::new(),
            contention_pairs: Vec::new(),
        };
        let mut html = String::new();
        render_runtime_cluster(&mut html, "<title>", "a & b", &[metric]);
        let empty_metric = RuntimeMetric {
            id: "empty".into(),
            label: "Empty".into(),
            value: "0".into(),
            magnitude: 0,
            detail: String::new(),
            threads: BTreeMap::new(),
            stacks: Vec::new(),
            contention_pairs: Vec::new(),
        };
        render_runtime_stack_panel(&mut html, &empty_metric, &HashMap::new());

        assert!(html.contains("&lt;title&gt;"));
        assert!(html.contains("width:0.00%"));
        assert!(html.contains("No retained call stacks"));
    }

    #[test]
    fn runtime_stack_panel_renders_plural_contention_without_holder() {
        let metric = RuntimeMetric {
            id: "metric".into(),
            label: "Metric".into(),
            value: "2".into(),
            magnitude: 2,
            detail: String::new(),
            threads: BTreeMap::from([(1, 2)]),
            stacks: vec![RuntimeStackTotal {
                kind: "MutexContention".into(),
                stack: Vec::new(),
                count: 2,
            }],
            contention_pairs: vec![RuntimeContentionPair {
                kind: "Mutex contention".into(),
                blocked: Vec::new(),
                holder: None,
                count: 2,
            }],
        };
        let mut html = String::new();

        render_runtime_stack_panel(&mut html, &metric, &HashMap::new());

        assert!(html.contains("2 observations"));
        assert!(html.contains("Holder stack unavailable"));
        assert!(html.contains("No captured stack"));
    }

    #[test]
    fn runtime_object_details_cover_every_primitive_type() {
        let events = [
            runtime_event(1, 1, RuntimeEventKind::ArcDeref, 1, &[]),
            runtime_event(1, 2, RuntimeEventKind::ArcClone, 1, &[]),
            runtime_event(2, 3, RuntimeEventKind::ArcRelocate, 1, &[]),
            runtime_event(1, 4, RuntimeEventKind::MutexAccess, 1, &[]),
            runtime_event(1, 5, RuntimeEventKind::MutexContention, 1, &[]),
            runtime_event(1, 6, RuntimeEventKind::LockPoisoned, 1, &[]),
            runtime_event(1, 7, RuntimeEventKind::LockPoisonObserved, 1, &[]),
            runtime_event(1, 8, RuntimeEventKind::LockPoisonCleared, 1, &[]),
            runtime_event(1, 9, RuntimeEventKind::RwLockReadAccess, 1, &[]),
            runtime_event(1, 10, RuntimeEventKind::RwLockWriteAccess, 1, &[]),
            runtime_event(1, 11, RuntimeEventKind::RwLockReadContention, 1, &[]),
            runtime_event(1, 12, RuntimeEventKind::RwLockWriteContention, 1, &[]),
            runtime_event(1, 13, RuntimeEventKind::BarrierAccess, 1, &[]),
            runtime_event(1, 14, RuntimeEventKind::BarrierContention, 1, &[]),
            runtime_event(1, 15, RuntimeEventKind::BarrierRelease, 1, &[]),
            runtime_event(1, 16, RuntimeEventKind::CondvarAccess, 1, &[]),
            runtime_event(1, 17, RuntimeEventKind::CondvarContention, 1, &[]),
            runtime_event(1, 18, RuntimeEventKind::CondvarNotify, 1, &[]),
            runtime_event(1, 19, RuntimeEventKind::OnceAccess, 1, &[]),
            runtime_event(1, 20, RuntimeEventKind::OnceContention, 1, &[]),
            runtime_event(1, 21, RuntimeEventKind::OnceInitialize, 1, &[]),
            runtime_event(1, 22, RuntimeEventKind::ChannelSend, 1, &[]),
            runtime_event(1, 23, RuntimeEventKind::ChannelReceive, 1, &[]),
            runtime_event(1, 24, RuntimeEventKind::ChannelSendContention, 1, &[]),
            runtime_event(1, 25, RuntimeEventKind::ChannelReceiveContention, 1, &[]),
            runtime_event(1, 26, RuntimeEventKind::ChannelHighWatermark, 1, &[]),
            runtime_event(1, 27, RuntimeEventKind::ChannelClose, 1, &[]),
        ];
        let event_refs = events.iter().collect::<Vec<_>>();

        assert_eq!(
            ["arc", "mutex", "rwlock", "barrier", "condvar", "once", "channel", "unknown",]
                .map(|prefix| runtime_object_detail(prefix, &event_refs, 2))
                .map(|detail| detail.is_empty()),
            [false; 8]
        );

        let metrics = runtime_object_metrics(
            "barrier",
            "Barrier",
            &events,
            &[RuntimeEventKind::BarrierAccess, RuntimeEventKind::BarrierContention],
            &[RuntimeEventKind::BarrierAccess],
            &[RuntimeEventKind::BarrierAccess],
        );
        assert_eq!(metrics.len(), 1);

        let ranked = runtime_object_metrics(
            "arc",
            "Arc",
            &[
                runtime_event(1, 30, RuntimeEventKind::ArcDeref, 10, &[]),
                runtime_event(1, 31, RuntimeEventKind::ArcDeref, 11, &[]),
                runtime_event(1, 32, RuntimeEventKind::ArcClone, 11, &[]),
            ],
            &[RuntimeEventKind::ArcDeref, RuntimeEventKind::ArcClone],
            &[RuntimeEventKind::ArcDeref, RuntimeEventKind::ArcClone],
            &[RuntimeEventKind::ArcDeref],
        );
        assert_eq!((ranked[0].magnitude, ranked[0].stacks.len()), (1, 2));
    }

    #[test]
    fn runtime_contention_tracks_mutex_and_rwlock_holders() {
        let events = vec![
            runtime_event(1, 1, RuntimeEventKind::MutexAccess, 1, &[10]),
            runtime_event(1, 2, RuntimeEventKind::MutexContention, 1, &[20]),
            runtime_event(1, 3, RuntimeEventKind::MutexRelease, 1, &[]),
            runtime_event(1, 4, RuntimeEventKind::RwLockWriteAccess, 1, &[30]),
            runtime_event(1, 5, RuntimeEventKind::RwLockReadContention, 1, &[40]),
            runtime_event(1, 6, RuntimeEventKind::RwLockWriteRelease, 1, &[]),
            runtime_event(1, 7, RuntimeEventKind::RwLockReadAccess, 1, &[50]),
            runtime_event(1, 8, RuntimeEventKind::RwLockWriteContention, 1, &[60]),
            runtime_event(1, 9, RuntimeEventKind::RwLockReadRelease, 1, &[]),
        ];
        let references = events.iter().collect::<Vec<_>>();

        let pairs = runtime_contention_pairs(&references);

        assert_eq!(pairs.len(), 3);
        assert!(pairs.iter().all(|pair| pair.holder.is_some()));

        let second_thread = runtime_event(2, 10, RuntimeEventKind::MutexContention, 1, &[70]);
        let unordered_write = runtime_event(2, 11, RuntimeEventKind::RwLockWriteContention, 1, &[80]);
        let unordered = [references[0], &second_thread, &unordered_write];
        assert!(runtime_contention_pairs(&unordered).iter().all(|pair| pair.holder.is_none()));
    }

    #[test]
    fn runtime_frame_helpers_filter_recorder_frames_and_format_locations() {
        let lookups = [
            AddressLookup::from_fields(AddressLookupFields {
                address: 1,
                symbol: Some("performables::sync::mutex::lock".into()),
                filename: Some(r"C:\src\performables\src\sync\mutex.rs".into()),
                line: Some(10),
                column: None,
            }),
            AddressLookup::from_fields(AddressLookupFields {
                address: 2,
                symbol: Some("app::run::<u64>".into()),
                filename: Some(r"C:\repo\src\deep\app.rs".into()),
                line: Some(20),
                column: None,
            }),
            AddressLookup::from_fields(AddressLookupFields {
                address: 3,
                symbol: None,
                filename: None,
                line: None,
                column: None,
            }),
        ];
        let lookups = lookups.iter().map(|lookup| (lookup.address, lookup)).collect::<HashMap<_, _>>();

        assert_eq!(
            (
                runtime_stack_frames(&[1, 2, 3], &lookups),
                render_runtime_frames(&[]),
                format_runtime_frame(3, lookups.get(&3).copied()),
                format_runtime_frame(4, None),
                compact_runtime_symbol("outer::<inner<u8>>::run"),
                is_runtime_recorder_frame("backtrace::trace"),
                is_runtime_recorder_frame("app::run"),
                ratio(1, 0),
                percentage(1, 4),
            ),
            (
                vec![
                    "app::run:: (repo\\src\\deep\\app.rs:20)".to_owned(),
                    "0x0000000000000003".to_owned(),
                ],
                "<p class=\"muted\">No captured stack.</p>".to_owned(),
                "0x0000000000000003".to_owned(),
                "0x0000000000000004".to_owned(),
                "outer::::run".to_owned(),
                true,
                false,
                0.0,
                25.0,
            )
        );
        assert!(render_runtime_party_stack(&[2], &lookups).contains("Full stack"));
    }

    #[test]
    fn source_inventory_distinguishes_specialized_and_generic_sources() {
        let mut html = String::new();
        render_sources(
            &mut html,
            &[
                seismograph::snapshot::SourceSnapshot {
                    id: seismograph_rallocator::source::ID,
                    name: "allocator<&".into(),
                    schema_version: 2,
                    data: vec![0; 1_024],
                },
                seismograph::snapshot::SourceSnapshot {
                    id: seismograph::snapshot::SourceId::new(999),
                    name: "custom".into(),
                    schema_version: 3,
                    data: Vec::new(),
                },
            ],
        );

        assert!(html.contains("allocator&lt;&amp;"));
        assert!(html.contains("Specialized allocator panels"));
        assert!(html.contains("Generic source metadata only"));
    }

    #[test]
    fn empty_regions_and_boundary_grids_render_safely() {
        let mut region = TopologyRegion::default();
        region.slice_bytes = 1;
        let mut html = String::new();

        render_region(&mut html, &region, None);

        assert!(html.contains("0.00%"));
        assert!(slice_runs(&[], 0).is_empty());
        assert_eq!(grid_width(16_384), 128);
        assert_eq!(histogram_label(0), "0 B");
    }

    #[test]
    fn physical_topology_renders_one_selectable_region_at_a_time() {
        let mut snapshot = Snapshot::new(Version::new(1, 0, 0));
        let mut first = TopologyRegion::default();
        first.region_index = 3;
        first.slice_bytes = 1;
        let mut second = TopologyRegion::default();
        second.region_index = 7;
        second.slice_bytes = 1;
        snapshot.topology = vec![first, second];

        let html = render_html(&snapshot);

        assert!(html.contains("<option value=\"3\" selected>Region #3</option>"));
        assert!(html.contains("<option value=\"7\">Region #7</option>"));
        assert!(html.contains("<div class=\"topology-region\" data-region=\"3\">"));
        assert!(html.contains("<div class=\"topology-region\" data-region=\"7\" hidden>"));
    }

    #[test]
    fn runtime_events_render_with_sampling_loss_and_stack_details() {
        let mut snapshot = Snapshot::new(Version::new(1, 0, 0));
        snapshot.runtime_events = Some(Events {
            clock: EventClock::ProcessMonotonic,
            total_events: 5,
            lost_events: 1,
            recording: seismograph::recorder::RecordingPolicies {
                allocations: seismograph::recorder::RecordingPolicy {
                    event_sampling: seismograph::recorder::EventSampling::one_in(8).expect("valid test sampling"),
                    ..Default::default()
                },
                ..Default::default()
            },
            threads: vec![ThreadLog {
                thread_id: seismograph::recorder::thread::ThreadId::new(7),
                total_events: 5,
                lost_events: 1,
                name: "worker".to_owned(),
            }],
            events: vec![
                Event {
                    thread_id: seismograph::recorder::thread::ThreadId::new(7),
                    sequence: seismograph::recorder::event::EventSequence::new(1),
                    timestamp: EventTimestamp::from_ticks(1),
                    kind: RuntimeEventKind::MutexAccess,
                    payload: EventPayload::Object(seismograph::recorder::event::ObjectId::new(0x1234)),
                    call_stack: vec![seismograph::recorder::event::Address::new(0x9999)],
                },
                Event {
                    thread_id: seismograph::recorder::thread::ThreadId::new(7),
                    sequence: seismograph::recorder::event::EventSequence::new(2),
                    timestamp: EventTimestamp::from_ticks(2),
                    kind: RuntimeEventKind::MutexContention,
                    payload: EventPayload::Object(seismograph::recorder::event::ObjectId::new(0x1234)),
                    call_stack: vec![seismograph::recorder::event::Address::new(0x5678)],
                },
                Event {
                    thread_id: seismograph::recorder::thread::ThreadId::new(7),
                    sequence: seismograph::recorder::event::EventSequence::new(3),
                    timestamp: EventTimestamp::from_ticks(3),
                    kind: RuntimeEventKind::LockPoisoned,
                    payload: EventPayload::Object(seismograph::recorder::event::ObjectId::new(0x1234)),
                    call_stack: vec![seismograph::recorder::event::Address::new(0x9999)],
                },
                Event {
                    thread_id: seismograph::recorder::thread::ThreadId::new(7),
                    sequence: seismograph::recorder::event::EventSequence::new(4),
                    timestamp: EventTimestamp::from_ticks(4),
                    kind: RuntimeEventKind::LockPoisonObserved,
                    payload: EventPayload::Object(seismograph::recorder::event::ObjectId::new(0x1234)),
                    call_stack: vec![seismograph::recorder::event::Address::new(0x5678)],
                },
                Event {
                    thread_id: seismograph::recorder::thread::ThreadId::new(7),
                    sequence: seismograph::recorder::event::EventSequence::new(5),
                    timestamp: EventTimestamp::from_ticks(5),
                    kind: RuntimeEventKind::LockPoisonCleared,
                    payload: EventPayload::Object(seismograph::recorder::event::ObjectId::new(0x1234)),
                    call_stack: vec![seismograph::recorder::event::Address::new(0x5678)],
                },
            ],
        });
        snapshot.addresses.push(AddressLookup::from_fields(AddressLookupFields {
            address: 0x5678,
            symbol: Some("app::waiting_for_mutex".to_owned()),
            filename: Some("mutex.rs".to_owned()),
            line: Some(42),
            column: Some(7),
        }));
        snapshot.addresses.push(AddressLookup::from_fields(AddressLookupFields {
            address: 0x9999,
            symbol: Some("app::holding_the_mutex".to_owned()),
            filename: Some("holder.rs".to_owned()),
            line: Some(17),
            column: None,
        }));

        let html = render_html(&snapshot);

        assert!(html.contains("Runtime ownership and lock telemetry"));
        assert!(html.contains("allocation sampling recorded one in 8 objects"));
        assert!(html.contains("runtime-clusters"));
        assert!(html.contains("<h3>Arc</h3>"));
        assert!(html.contains("<h3>Mutex</h3>"));
        assert!(html.contains("<h3>RwLock</h3>"));
        assert!(html.contains("runtime-mutex-0000000000001234"));
        assert!(!html.contains("runtime-rwlock-0000000000001234"));
        assert!(html.contains("#1 · Mutex 0x0000000000001234"));
        assert!(html.contains("1 poisonings · 1 observations · 1 clears"));
        assert!(html.contains("LockPoisoned"));
        assert!(html.contains("LockPoisonObserved"));
        assert!(html.contains("LockPoisonCleared"));
        assert!(html.contains("Thread participation: 1 thread"));
        assert!(html.contains("Select a graph item"));
        assert!(html.contains("Contention parties"));
        assert!(html.contains("Blocked acquisition"));
        assert!(html.contains("Current holder"));
        assert!(html.contains("app::waiting_for_mutex"));
        assert!(html.contains("app::holding_the_mutex"));
        assert!(html.contains("mutex.rs:42"));
    }
}
