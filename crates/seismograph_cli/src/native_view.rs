// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Shared native structure presentation for terminal and portable HTML reports.

#[cfg(test)]
pub(crate) mod fixture;

use std::fmt::Write as _;

use seismograph_rallocator::native::{Freshness, ObservationSource, Owner, Ranges, Snapshot};

pub(crate) const LIMITATIONS: &str = "Native capacity is NOT application-live memory. Last contributor is NOT the current lease holder. \
     Busy/unobserved owners are unknown, not zero. Bounded walks can omit slabs/ranges/messages. \
     Matching rounds mean contributed this round, not an exact current census; the first round may predate polling. \
     Ages are measured at capture. Large outstanding allocator ranges include pending/retained frees, not app-live counts. \
     Observations are independent, not a transactional heap census. Global cached ranges are NOT guaranteed \
     physically decommitted; discard failure leaves residency unknown. Batching budget is NOT pending bytes. \
     Event filters do not filter native inventory. Self-publication follows accepted recorded allocation/free operations only; \
     sampled-out events and merely enabled attempts do not contribute. Polls request the next round after collection; \
     explicit app requests are also supported, without timers or background workers.";

pub(crate) fn overview(snapshot: &Snapshot) -> Vec<String> {
    let current = snapshot
        .owners
        .iter()
        .filter(|owner| owner.freshness(snapshot) == Freshness::Current)
        .count();
    let idle = snapshot
        .owners
        .iter()
        .filter(|owner| owner.freshness(snapshot) == Freshness::IdleInspection)
        .count();
    let unknown = snapshot.owners.iter().filter(|owner| owner.observation.is_none()).count();
    let busy = snapshot
        .owners
        .iter()
        .filter(|owner| owner.source == ObservationSource::Busy)
        .count();
    let unavailable = snapshot
        .owners
        .iter()
        .filter(|owner| owner.source == ObservationSource::Unavailable)
        .count();
    let unavailable_suffix = if unavailable == 0 {
        String::new()
    } else {
        format!(" · unavailable {unavailable} (System slot allocation failed)")
    };
    vec![
        format!(
            "Native v4 · session {} · round {} · capture {} ns · publication {}",
            snapshot.session_id,
            snapshot.round,
            snapshot.captured_nanos,
            if snapshot.publication_enabled { "enabled" } else { "disabled" }
        ),
        format!(
            "Owners: {} / {} · inventory {} · contributed {current} · idle inspected {idle} · unknown {unknown} · busy {busy}{unavailable_suffix}",
            snapshot.owners.len(),
            snapshot.owner_count,
            completeness(snapshot.owners_complete)
        ),
        format!(
            "Global backend: cumulative object-range reservations {} B · pagemap VA reservation {} B",
            snapshot.global.reserved_bytes, snapshot.global.pagemap_reserved_bytes
        ),
        format!(
            "Growth: local boundary {} B · global refill ceiling {} B (larger individual requests possible)",
            snapshot.global.local_limit_bytes, snapshot.global.global_refill_bytes
        ),
        format!(
            "Global cached range capacity: {} B · {}",
            snapshot.global.ranges.observed_bytes(),
            completeness(snapshot.global.ranges.complete)
        ),
    ]
}

const fn completeness(complete: bool) -> &'static str {
    if complete { "complete" } else { "PARTIAL bounded walk" }
}

pub(crate) fn owner_label(snapshot: &Snapshot, owner: &Owner) -> String {
    format!(
        "0x{:x} · lease {} {} · age {} · {}{}",
        owner.id,
        owner.generation,
        if owner.leased { "active" } else { "idle" },
        observation_age(snapshot, owner),
        if owner.source == ObservationSource::Busy { "BUSY · " } else { "" },
        if owner.source == ObservationSource::Unavailable {
            "UNAVAILABLE · System slot allocation failed · state unknown"
        } else {
            owner.freshness(snapshot).label()
        }
    )
}

fn observation_age(snapshot: &Snapshot, owner: &Owner) -> String {
    owner.observation.as_ref().map_or_else(
        || "unknown (no observation)".into(),
        |observation| {
            snapshot
                .captured_nanos
                .checked_sub(observation.captured_nanos)
                .map_or_else(|| "unknown (newer than capture)".into(), |nanos| format!("{nanos} ns at capture"))
        },
    )
}

pub(crate) fn owner_lines(snapshot: &Snapshot, owner: &Owner) -> Vec<String> {
    let mut lines = vec![owner_label(snapshot, owner)];
    let Some(observation) = &owner.observation else {
        lines.push(if owner.source == ObservationSource::Unavailable {
            "Publication slot System allocation failed; state is UNKNOWN, not zero or merely never-observed.".into()
        } else {
            "No readable observation: classes, large ranges, caches and outgoing returns are UNKNOWN.".into()
        });
        return lines;
    };
    let contributor = if owner.source == ObservationSource::IdleInspection {
        "Collector idle inspection (no recorder contributor)".into()
    } else if observation.thread_id == 0 {
        "Last contributor recorder thread unavailable (not recorded)".into()
    } else {
        format!("Last contributor recorder thread {}", observation.thread_id)
    };
    let age = observation_age(snapshot, owner);
    lines.push(format!(
        "{contributor} · observed lease {} · session {} · round {} · capture {} ns · age {age}",
        observation.generation, observation.session_id, observation.round, observation.captured_nanos,
    ));
    lines.push(format!(
        "Small-class slab inventory: {}. Bars show observed slab counts, not object liveness.",
        completeness(observation.slabs_complete)
    ));
    let maximum = observation.classes.iter().map(|class| class.observed_slabs).max().unwrap_or(0);
    let absent_descriptors = observation.classes.iter().filter(|class| class.object_bytes == 0).count();
    if absent_descriptors != 0 {
        lines.push(format!(
            "{absent_descriptors} class descriptors unavailable; absent descriptors are not measured zero capacity."
        ));
    }
    for (index, class) in observation.classes.iter().enumerate().filter(|(_, class)| class.object_bytes != 0) {
        lines.push(format!("  class {index:2} · object {:>7} B · slab {:>8} B · capacity {:>6} · available {:>3} · empty {:>3} · observed {:>3} · fast {} {}",
            class.object_bytes, class.slab_bytes, class.capacity, class.available_slabs, class.empty_slabs,
            class.observed_slabs, if class.fast_nonempty { "ready" } else { "empty" }, bar(class.observed_slabs, maximum)));
    }
    range_lines(
        &mut lines,
        "Large outstanding native ranges (NOT application-live)",
        &observation.large,
    );
    lines.push("Outstanding allocator ranges include pending/retained frees; neither bytes nor counts are app-live.".into());
    range_lines(&mut lines, "Local backend reusable object ranges", &observation.local.ranges);
    range_lines(&mut lines, "Local metadata reusable ranges", &observation.local.metadata);
    lines.push(format!(
        "Local refill requested growth: {} B (cumulative growth state, NOT retained bytes)",
        observation.local.requested_bytes
    ));
    let remote = observation.remote;
    lines.push(format!(
        "Outgoing returns: {} open rings / {} objects · {} message buckets / {} messages / {} objects / {} class-rounded B · {}",
        remote.open_rings,
        remote.open_objects,
        remote.outgoing_lists,
        remote.messages,
        remote.message_objects,
        remote.message_bytes,
        completeness(remote.complete)
    ));
    lines.push(format!("Remaining batching budget: {} (NOT queued bytes)", remote.budget_remaining));
    lines.push(format!(
        "Incoming atomic queue: front 0x{:x} · back 0x{:x} · {} · pointers sampled, never traversed",
        remote.incoming_front, remote.incoming_back, "opaque endpoints; depth/emptiness UNKNOWN"
    ));
    lines.push(format!(
        "Sampled front != back: {} (potential work only; ready links NOT guaranteed; equality does NOT prove emptiness)",
        remote.incoming_front != remote.incoming_back
    ));
    lines
}

fn bar(count: u64, maximum: u64) -> String {
    let filled = if maximum == 0 {
        0
    } else {
        (u128::from(count) * 20 / u128::from(maximum)) as usize
    };
    format!("[{}{}]", "#".repeat(filled), ".".repeat(20 - filled))
}

fn range_lines(lines: &mut Vec<String>, title: &str, ranges: &Ranges) {
    lines.push(format!(
        "{title}: {} B observed · {}",
        ranges.observed_bytes(),
        completeness(ranges.complete)
    ));
    let maximum = ranges.counts.iter().copied().max().unwrap_or(0);
    for (index, count) in ranges.counts.iter().enumerate().filter(|(_, count)| **count != 0) {
        lines.push(format!("  2^{index:2} B · {count:>8} ranges {}", bar(*count, maximum)));
    }
}

pub(crate) fn render_html(snapshot: Option<&Snapshot>) -> String {
    let Some(snapshot) = snapshot else {
        return "<section><h2>Native v4 structure explorer</h2><p>Native allocator source unavailable. Owner coverage is unknown, not zero.</p></section>".into();
    };
    let mut html = String::from(
        "<section class=\"native-v4\"><style>\
        .native-v4 pre{overflow-x:auto;white-space:pre}\
        .native-v4 .current summary{border-left:4px solid #39a96b;padding-left:8px}\
        .native-v4 .stale summary{border-left:4px solid #dba340;padding-left:8px}\
        .native-v4 .unknown summary{border-left:4px solid #87909b;padding-left:8px}\
        .native-v4 .unavailable summary{border-left:4px solid #d74242;padding-left:8px}\
        </style><h2>Native v4 structure explorer</h2><pre>",
    );
    for line in overview(snapshot) {
        writeln!(html, "{}", escape(&line)).expect("formatting into String cannot fail");
    }
    let mut ranges = Vec::new();
    range_lines(
        &mut ranges,
        "Global backend cached ranges (physical residency unknown)",
        &snapshot.global.ranges,
    );
    for line in ranges {
        writeln!(html, "{}", escape(&line)).expect("formatting into String cannot fail");
    }
    html.push_str("</pre><p>");
    html.push_str(LIMITATIONS);
    html.push_str("</p><div class=\"native-owners\">");
    for owner in &snapshot.owners {
        let coverage = if owner.source == ObservationSource::Unavailable {
            "unavailable"
        } else if owner.source == ObservationSource::Busy {
            "stale"
        } else {
            match owner.freshness(snapshot) {
                Freshness::Current | Freshness::IdleInspection => "current",
                Freshness::Unknown => "unknown",
                _ => "stale",
            }
        };
        write!(
            html,
            "<details class=\"{coverage}\"><summary>{}</summary><pre>",
            escape(&owner_label(snapshot, owner))
        )
        .expect("formatting into String cannot fail");
        for line in owner_lines(snapshot, owner).iter().skip(1) {
            writeln!(html, "{}", escape(line)).expect("formatting into String cannot fail");
        }
        html.push_str("</pre></details>");
    }
    html.push_str("</div></section>");
    html
}

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
