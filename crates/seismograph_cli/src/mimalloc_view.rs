// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Mimalloc state presentation for the terminal viewer.

#[cfg(test)]
pub(crate) mod fixture;

use seismograph_rallocator::mimalloc::{ClassState, ObservationSource, Owner, State};
use seismograph_rallocator::native::Freshness;
use seismograph_rallocator::snapshot::{BackendState, Header, Snapshot};

// Matches the producer's ARENA_BINS. Other page bins do not participate in its abandoned index.
const REGULAR_ABANDONED_BIN_COUNT: usize = 61;

pub(crate) const fn parts(snapshot: &Snapshot) -> Option<(&Header, &State)> {
    match &snapshot.backend {
        BackendState::Mimalloc(state) => Some((&snapshot.header, state)),
        BackendState::Snmalloc(_) => None,
    }
}

pub(crate) const fn coverage(complete: bool) -> &'static str {
    if complete { "complete" } else { "PARTIAL / bounded, truncated" }
}

pub(crate) fn overview(header: &Header, state: &State) -> Vec<String> {
    let active = state.owners.iter().filter(|owner| owner.active).count();
    let exited = state.owners.len() - active;
    let contributed = state
        .owners
        .iter()
        .filter(|owner| owner.freshness(header) == Freshness::Current)
        .count();
    let busy = state.owners.iter().filter(|owner| owner.source == ObservationSource::Busy).count();
    let unobserved = state
        .owners
        .iter()
        .filter(|owner| owner.source == ObservationSource::Unobserved)
        .count();
    let unavailable = state
        .owners
        .iter()
        .filter(|owner| owner.source == ObservationSource::Unavailable)
        .count();
    vec![
        format!(
            "Mimalloc · session {} · round {} · capture {} ns · publication {}",
            header.session_id,
            header.round,
            header.captured_nanos,
            if header.publication_enabled { "enabled" } else { "disabled" }
        ),
        format!(
            "Heap owners: {} / {} · inventory {} · active {active} · exited {exited}",
            state.owners.len(),
            header.owner_count,
            coverage(header.owners_complete),
        ),
        format!("Contributed this round {contributed} · busy {busy} · unobserved {unobserved} · unavailable {unavailable}"),
        format!(
            "Arena scan: {} arenas · {} · OS reservation {} B (arenas + direct OS, metadata already included)",
            state.global.arena_count,
            coverage(state.global.arenas_complete),
            os_reserved_bytes(state),
        ),
    ]
}

pub(crate) fn os_reserved_bytes(state: &State) -> u128 {
    u128::from(state.global.reserved_bytes) + u128::from(state.global.os_reserved_bytes)
}

pub(crate) fn global_lines(state: &State) -> Vec<String> {
    let global = &state.global;
    let mut lines = vec![
        format!("Arena scan: {} arenas · {}", global.arena_count, coverage(global.arenas_complete)),
        format!(
            "OS reservation: {} B (arenas + direct OS, metadata already included)",
            os_reserved_bytes(state)
        ),
        format!("Arena reservation: {} B", global.reserved_bytes),
        format!(
            "Arena bitmap commitment: {} B (includes tracked arena metadata)",
            global.committed_bytes
        ),
        format!("Arena metadata: {} B (already included, do not add again)", global.metadata_bytes),
        format!("Arena free slices: {}", global.free_slices),
        format!(
            "Pending purge slices: {} (bits awaiting an attempt, not guaranteed purgeable)",
            global.pending_purge_slices
        ),
        format!("Direct OS reservations: {}", global.os_allocations),
        format!("Direct OS reserved: {} B (includes metadata and padding)", global.os_reserved_bytes),
        format!(
            "Direct OS data committed: {} B (excludes metadata commitment)",
            global.os_committed_bytes
        ),
        "Total OS commitment: UNKNOWN (direct OS metadata commitment is not recorded)".into(),
        "Resident / swapped memory: UNKNOWN. Bitmap commit is NOT physical residency.".into(),
        "Global scans read independent atomics. Values are not a simultaneous census.".into(),
        "INDEXED regular abandoned pages by class (bins 0..60, not live objects, excluded from owner queues):".into(),
    ];
    let mut any = false;
    for (index, pages) in global
        .abandoned_pages
        .iter()
        .take(REGULAR_ABANDONED_BIN_COUNT)
        .enumerate()
        .filter(|(_, pages)| **pages != 0)
    {
        lines.push(format!("  class {index:2}: {pages} INDEXED regular abandoned pages"));
        any = true;
    }
    if !any {
        lines.push("  All INDEXED regular abandoned-page counts (bins 0..60) are zero in these independent observations.".into());
    }
    lines.push(
        "Unindexed abandoned pages: UNKNOWN (huge/singleton pages and some OS-backed pages). Zeros do not prove their absence.".into(),
    );
    lines
}

pub(crate) fn freshness(header: &Header, owner: &Owner) -> &'static str {
    if !owner.active {
        return if owner.observation.is_some() {
            "EXITED / historical observation"
        } else {
            "EXITED / state unknown"
        };
    }
    match owner.freshness(header) {
        Freshness::Current => "contributed this round",
        Freshness::PreviousSession => "older session",
        Freshness::PreviousRound => "older round",
        Freshness::NewerThanCapture => "newer than capture",
        Freshness::Unknown => "unknown / unobserved",
        Freshness::PreviousLease => "historical observation",
        Freshness::IdleInspection => "idle inspection",
    }
}

pub(crate) const fn source_label(source: ObservationSource) -> &'static str {
    match source {
        ObservationSource::Published => "published",
        ObservationSource::Busy => "BUSY",
        ObservationSource::Unobserved => "UNOBSERVED",
        ObservationSource::Unavailable => "UNAVAILABLE",
    }
}

pub(crate) fn owner_label(header: &Header, owner: &Owner) -> String {
    format!(
        "Heap #{} · {} · {} · {}",
        owner.id,
        if owner.active { "active" } else { "exited" },
        source_label(owner.source),
        freshness(header, owner)
    )
}

fn age(header: &Header, captured_nanos: u64) -> String {
    header
        .captured_nanos
        .checked_sub(captured_nanos)
        .map_or_else(|| "unknown (newer than capture)".into(), |nanos| format!("{nanos} ns at capture"))
}

pub(crate) fn owner_lines(header: &Header, owner: &Owner) -> Vec<String> {
    let mut lines = vec![owner_label(header, owner)];
    match owner.source {
        ObservationSource::Busy => {
            lines.push("Publication slot BUSY. Any readable copy below is retained evidence, not a new observation.".into());
        }
        ObservationSource::Unavailable => {
            lines.push("Diagnostic storage allocation failed. State is UNAVAILABLE, not measured zero.".into());
        }
        ObservationSource::Unobserved => {
            lines.push("The owner did not publish readable state. State is UNOBSERVED, not measured zero.".into());
        }
        ObservationSource::Published => {}
    }
    let Some(observation) = &owner.observation else {
        lines.push("Pages, block capacity, used blocks and free lists are UNKNOWN, not zero.".into());
        return lines;
    };
    if !owner.active {
        lines.push(
            "Heap cleanup ended. This copy is historical and does not describe an active heap. Heap identifiers are never reused.".into(),
        );
    }
    lines.push(format!(
        "Contributor recorder thread {} · session {} · round {} · capture {} ns · age {}",
        observation.thread_id,
        observation.session_id,
        observation.round,
        observation.captured_nanos,
        age(header, observation.captured_nanos),
    ));
    lines.push(format!(
        "Page walk: {} · free-list walk: {}",
        coverage(observation.pages_complete),
        coverage(observation.free_lists_complete),
    ));
    lines.push("Capacity is NOT application-live memory. Used blocks include uncollected pending remote frees.".into());
    lines.push("Queued pages exclude abandoned pages. Page counts and free-list counts have separate visit budgets.".into());
    lines
}

pub(crate) fn class_label(owner: &Owner, index: usize) -> String {
    let Some(observation) = &owner.observation else {
        return format!("Class {index:2} · UNKNOWN");
    };
    let class = &observation.classes[index];
    format!(
        "Class {index:2} · block {} · pages {}/{} · used {} / capacity {}",
        block_bytes(class),
        class.observed_pages,
        class.queued_pages,
        class.used_blocks,
        class.capacity_blocks
    )
}

fn block_bytes(class: &ClassState) -> String {
    if class.block_bytes == 0 {
        "variable / no fixed size".into()
    } else {
        format!("{} B", class.block_bytes)
    }
}

pub(crate) fn class_lines(header: &Header, owner: &Owner, index: usize) -> Vec<String> {
    let mut lines = owner_lines(header, owner);
    let Some(observation) = &owner.observation else {
        return lines;
    };
    let class = &observation.classes[index];
    lines.extend([
        format!("Class {index}"),
        format!("Block size: {}", block_bytes(class)),
        if class.page_bytes == 0 {
            "Backing page size: variable / no ordinary page size".into()
        } else {
            format!("Backing page size: {} B", class.page_bytes)
        },
        format!("Queued pages: {} (excludes abandoned pages)", class.queued_pages),
        format!(
            "Observed pages: {} · {}",
            class.observed_pages,
            coverage(observation.pages_complete)
        ),
        "Block counters below describe visited pages only, not omitted pages.".into(),
        format!("Reserved blocks: {} (includes uninitialized slots)", class.reserved_blocks),
        format!("Initialized capacity blocks: {}", class.capacity_blocks),
        format!("Used blocks: {} (includes uncollected pending remote frees)", class.used_blocks),
        format!(
            "Allocation-ready free blocks: {} · {}",
            class.free_blocks,
            coverage(observation.free_lists_complete)
        ),
        format!(
            "Locally deferred free blocks: {} · {}",
            class.local_free_blocks,
            coverage(observation.free_lists_complete)
        ),
        format!("Retired pages: {} (active retirement countdown)", class.retired_pages),
    ]);
    lines
}

#[cfg(test)]
mod tests {
    use seismograph_rallocator::snapshot::BackendState;

    use super::{class_lines, fixture, global_lines, parts};

    fn class_walks(pages_complete: bool, free_lists_complete: bool) -> Vec<String> {
        let mut snapshot = fixture::snapshot();
        let BackendState::Mimalloc(state) = &mut snapshot.backend else {
            unreachable!();
        };
        let observation = state.owners[0].observation.as_mut().unwrap();
        observation.pages_complete = pages_complete;
        observation.free_lists_complete = free_lists_complete;
        let source = fixture::source(&snapshot);
        let decoded = seismograph_rallocator::snapshot::decode(&source.data).unwrap();
        let (header, state) = parts(&decoded).unwrap();
        class_lines(header, &state.owners[0], 1)
            .into_iter()
            .filter(|line| {
                line.starts_with("Observed pages:")
                    || line.starts_with("Allocation-ready free blocks:")
                    || line.starts_with("Locally deferred free blocks:")
            })
            .collect()
    }

    #[test]
    fn complete_page_walk_does_not_hide_truncated_free_lists() {
        let lines = class_walks(true, false);

        assert_eq!(
            lines,
            [
                "Observed pages: 2 · complete",
                "Allocation-ready free blocks: 20 · PARTIAL / bounded, truncated",
                "Locally deferred free blocks: 10 · PARTIAL / bounded, truncated",
            ],
        );
    }

    #[test]
    fn complete_free_lists_do_not_hide_a_truncated_page_walk() {
        let lines = class_walks(false, true);

        assert_eq!(
            lines,
            [
                "Observed pages: 2 · PARTIAL / bounded, truncated",
                "Allocation-ready free blocks: 20 · complete",
                "Locally deferred free blocks: 10 · complete",
            ],
        );
    }

    #[test]
    fn reservations_use_wide_arithmetic_without_counting_metadata_twice() {
        let mut snapshot = fixture::snapshot();
        let BackendState::Mimalloc(state) = &mut snapshot.backend else {
            unreachable!();
        };
        state.global.reserved_bytes = u64::MAX;
        state.global.os_reserved_bytes = u64::MAX;
        state.global.metadata_bytes = 999;
        let source = fixture::source(&snapshot);
        let decoded = seismograph_rallocator::snapshot::decode(&source.data).unwrap();
        let (_, state) = parts(&decoded).unwrap();

        let lines = global_lines(state);

        assert_eq!(
            lines[1],
            "OS reservation: 36893488147419103230 B (arenas + direct OS, metadata already included)"
        );
    }
}
