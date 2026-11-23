// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use seismograph_rallocator::native::{Ranges, Snapshot};
use seismograph_rallocator::snapshot::BackendState;
use tonic::Status;

use super::{CapturedSnapshot, Field, Page, Row, at, bad_path, fields, page, paginate, row};

pub(super) fn native(snapshot: &CapturedSnapshot, path: &[usize], offset: usize) -> Result<Page, Status> {
    let snapshot = snapshot.native.as_ref().ok_or_else(bad_path)?;
    let global = &snapshot.global;
    match *path {
        [] => Ok(native_overview(snapshot, offset)),
        [1] => Ok(ranges(&global.ranges, offset)),
        [2] => Ok(page(&snapshot.owners, offset, |owner| Row {
            has_children: owner.observation.is_some(),
            ..row(
                format!("Owner {}", owner.id),
                fields![
                    "State" => if owner.leased { "Active" } else { "Idle" },
                    "Source" => format_args!("{:?}", owner.source), "Freshness" => owner.freshness(snapshot).label(),
                    "Lease generation" => owner.generation,
                ],
            )
        })),
        [2, owner] | [2, owner, _] => {
            let data = at(&snapshot.owners, owner)?.observation.as_ref().ok_or_else(bad_path)?;
            match path.get(2) {
                None => Ok(paginate(
                    [
                        row(
                            "Observation",
                            fields!["Session" => data.session_id, "Round" => data.round, "Lease generation" => data.generation,
                        "Contributor" => data.thread_id, "Capture time (ns)" => data.captured_nanos],
                        ),
                        Row {
                            has_children: true,
                            ..row("Small classes", fields!["Complete slab walk" => data.slabs_complete])
                        },
                        Row {
                            has_children: true,
                            ..row("Large ranges", fields!["Complete range walk" => data.large.complete])
                        },
                        Row {
                            has_children: true,
                            ..row(
                                "Local range cache",
                                fields![
                        "Growth bytes" => data.local.requested_bytes, "Complete range walk" => data.local.ranges.complete],
                            )
                        },
                        Row {
                            has_children: true,
                            ..row("Metadata cache", fields!["Complete range walk" => data.local.metadata.complete])
                        },
                        row(
                            "Remote returns",
                            fields!["Open rings" => data.remote.open_rings, "Open objects" => data.remote.open_objects,
                        "Outgoing lists" => data.remote.outgoing_lists, "Messages" => data.remote.messages,
                        "Message objects" => data.remote.message_objects, "Message capacity bytes" => data.remote.message_bytes,
                        "Complete walk" => data.remote.complete, "Batch budget" => data.remote.budget_remaining,
                        "Incoming work possible" => (data.remote.incoming_front != data.remote.incoming_back)],
                        ),
                    ]
                    .into_iter(),
                    offset,
                    std::convert::identity,
                )),
                Some(1) => Ok(paginate(data.classes.iter().enumerate(), offset, |(index, class)| {
                    row(
                        format!("Class {index}"),
                        fields!["Object bytes" => class.object_bytes, "Slab bytes" => class.slab_bytes,
                        "Objects per slab" => class.capacity, "Available slabs" => class.available_slabs, "Empty slabs" => class.empty_slabs,
                        "Observed slabs" => class.observed_slabs, "Fast list ready" => class.fast_nonempty],
                    )
                })),
                Some(2) => Ok(ranges(&data.large, offset)),
                Some(3) => Ok(ranges(&data.local.ranges, offset)),
                Some(4) => Ok(ranges(&data.local.metadata, offset)),
                _ => Err(bad_path()),
            }
        }
        _ => Err(bad_path()),
    }
}

fn native_overview(snapshot: &Snapshot, offset: usize) -> Page {
    let global = &snapshot.global;
    let rows = [
        row(
            "Virtual reservation",
            fields![
                "OS reserved bytes" => u128::from(global.reserved_bytes) + u128::from(global.pagemap_reserved_bytes),
                "Allocator range bytes" => global.reserved_bytes, "Page-map bytes" => global.pagemap_reserved_bytes,
                "Physical residency" => "Unknown",
            ],
        ),
        Row {
            has_children: true,
            ..row(
                "Shared backend",
                fields![
                    "Local boundary bytes" => global.local_limit_bytes, "Refill ceiling bytes" => global.global_refill_bytes,
                    "Complete range walk" => global.ranges.complete,
                ],
            )
        },
        Row {
            has_children: !snapshot.owners.is_empty(),
            ..row(
                "Owner frontends",
                fields![
                    "Owner count" => snapshot.owner_count, "Observed owners" => snapshot.owners.len(),
                    "Complete inventory" => snapshot.owners_complete, "Publication enabled" => snapshot.publication_enabled,
                    "Session" => snapshot.session_id, "Round" => snapshot.round,
                ],
            )
        },
    ];
    paginate(rows.into_iter(), offset, std::convert::identity)
}

fn ranges(data: &Ranges, offset: usize) -> Page {
    let present = data.counts.iter().enumerate().filter(|(_, count)| **count != 0);
    Page {
        total: u32::try_from(present.clone().count()).unwrap_or(u32::MAX),
        rows: present
            .skip(offset)
            .take(super::PAGE_SIZE)
            .map(|(index, count)| {
                row(
                    format!("2^{index} byte ranges"),
                    fields!["Observed ranges" => count, "Observed capacity bytes" => u128::from(*count) << index],
                )
            })
            .collect(),
        ..Page::default()
    }
}

pub(super) fn mimalloc(snapshot: &CapturedSnapshot, path: &[usize], offset: usize) -> Result<Page, Status> {
    let snapshot = snapshot.mimalloc.as_ref().ok_or_else(bad_path)?;
    let BackendState::Mimalloc(data) = &snapshot.backend else {
        return Err(bad_path());
    };
    let global = &data.global;
    let header = &snapshot.header;
    match *path {
        [] => Ok(paginate([
            row("Arena and direct OS reservations", fields![
                "Total reserved bytes" => u128::from(global.reserved_bytes) + u128::from(global.os_reserved_bytes),
                "Arena reserved bytes" => global.reserved_bytes, "Arena committed bytes" => global.committed_bytes,
                "Direct OS reserved bytes" => global.os_reserved_bytes, "Direct OS committed data bytes" => global.os_committed_bytes,
                "Arenas" => global.arena_count, "Arena metadata bytes (included)" => global.metadata_bytes,
                "Free slices" => global.free_slices, "Pending purge slices" => global.pending_purge_slices,
                "Complete arena scan" => global.arenas_complete, "Physical residency" => "Unknown",
            ]),
            Row { has_children: true, ..row("Indexed abandoned pages", fields!["Coverage" => "Regular bins only. Not a live object count."]) },
            Row { has_children: !data.owners.is_empty(), ..row("Heap lifetimes", fields![
                "Owner count" => header.owner_count, "Observed owners" => data.owners.len(), "Complete inventory" => header.owners_complete,
                "Publication enabled" => header.publication_enabled, "Session" => header.session_id, "Round" => header.round,
            ]) },
        ].into_iter(), offset, std::convert::identity)),
        [1] => Ok(paginate(
            global.abandoned_pages.iter().take(seismograph_rallocator::mimalloc::ABANDONED_CLASS_COUNT).enumerate(),
            offset,
            |(index, count)| row(format!("Bin {index}"), fields!["Indexed pages" => count]),
        )),
        [2] => Ok(page(&data.owners, offset, |owner| {
            let mut stats = fields!["State" => if owner.active { "Active" } else { "Exited" },
                "Source" => format_args!("{:?}", owner.source), "Freshness" => owner.freshness(header).label()];
            if let Some(observation) = owner.observation.as_ref() {
                stats.extend(fields!["Session" => observation.session_id, "Round" => observation.round,
                    "Contributor" => observation.thread_id, "Capture time (ns)" => observation.captured_nanos,
                    "Complete page walk" => observation.pages_complete, "Complete free-list walk" => observation.free_lists_complete]);
            }
            Row { has_children: owner.observation.is_some(), ..row(format!("Heap {}", owner.id), stats) }
        })),
        [2, owner] => {
            let data = at(&data.owners, owner)?.observation.as_ref().ok_or_else(bad_path)?;
            Ok(paginate(data.classes.iter().enumerate(), offset, |(index, class)| {
                row(format!("Bin {index}"), fields!["Block bytes" => class.block_bytes, "Page bytes" => class.page_bytes,
                    "Queued pages" => class.queued_pages, "Observed pages" => class.observed_pages, "Reserved blocks" => class.reserved_blocks,
                    "Initialized blocks" => class.capacity_blocks, "Used blocks" => class.used_blocks,
                    "Free blocks" => class.free_blocks, "Locally deferred blocks" => class.local_free_blocks, "Retired pages" => class.retired_pages])
            }))
        }
        _ => Err(bad_path()),
    }
}
