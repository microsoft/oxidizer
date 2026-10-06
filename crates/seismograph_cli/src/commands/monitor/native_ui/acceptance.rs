// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use super::tests::render;
use super::*;

#[test]
fn owner_selection_survives_prepend_reordering_and_back_navigation() {
    let mut snapshot = crate::native_view::fixture::snapshot();
    let mut nav = Navigation::default();
    nav.key(KeyCode::Down, Some(&snapshot));
    nav.key(KeyCode::Down, Some(&snapshot));
    nav.key(KeyCode::Enter, Some(&snapshot));
    nav.key(KeyCode::Enter, Some(&snapshot));
    nav.key(KeyCode::End, Some(&snapshot));
    nav.key(KeyCode::Enter, Some(&snapshot));
    let selected = nav.selected_owner;
    let owner = snapshot.owners[1];
    snapshot.owners.insert(0, Owner { id: 0x00ab_cdef, ..owner });
    snapshot.owners.swap(1, 3);
    nav.reconcile(Some(&snapshot));
    assert_eq!(snapshot.owners[nav.root - 2].id, selected.unwrap());
    assert_eq!((nav.depth, nav.class), (Depth::Class, 43));
    assert!(nav.key(KeyCode::Backspace, None));
    assert_eq!((nav.selected_owner, nav.depth), (selected, Depth::Classes));
    snapshot.owners.retain(|owner| Some(owner.id) != selected);
    nav.reconcile(Some(&snapshot));
    assert_eq!((nav.root, nav.selected_owner, nav.depth), (0, None, Depth::Root));
}

#[test]
fn inventory_completeness_and_evidence_counts_are_distinct() {
    let mut snapshot = crate::native_view::fixture::snapshot();
    snapshot.owners_complete = true;
    snapshot.owner_count = snapshot.owners.len() as u64;
    let output = render(Some(&snapshot), Navigation::default(), 140, 38);
    assert!(output.contains("7 / 7 [Complete]") && output.contains("Active 6") && output.contains("Idle 1"));
    assert!(output.contains("Observed 2  Older 2  Unknown 1  Busy 2  Unavailable 0"));
    assert!(!output.contains("7 / 7 [Observed]"));
    let geometry = slab_geometry(262_144, 4);
    assert_eq!(geometry.map(|line| line.chars().count()), [19, 19, 19]);
}

#[test]
fn human_capacity_labels_round_without_losing_virtual_reservation_precision() {
    assert_eq!(
        [bytes(6_373_376), bytes(274_894_684_160), bytes(16_777_216), bytes(274_877_906_944)],
        ["6.08 MiB", "256.02 GiB", "16.00 MiB", "256.00 GiB"]
    );
}

#[test]
fn oversized_small_capacity_is_explicitly_unknown_instead_of_overflowing() {
    let mut snapshot = crate::native_view::fixture::snapshot();
    snapshot.owners[0]
        .observation
        .as_mut()
        .unwrap()
        .classes
        .fill(seismograph_rallocator::native::ClassState {
            slab_bytes: u64::MAX,
            observed_slabs: u64::MAX,
            ..Default::default()
        });
    assert!(
        owner_overview(&snapshot, &snapshot.owners[0])
            .iter()
            .any(|line| line.contains("— [Unknown]"))
    );
}

#[test]
#[ignore = "requires a real recording copied to target/native-ui/actual-native.seismograph"]
fn write_actual_native_acceptance_screens() -> std::io::Result<()> {
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target").join("native-ui");
    let input = std::fs::read(directory.join("actual-native.seismograph"))?;
    let container = seismograph::snapshot::decode(&input).map_err(std::io::Error::other)?;
    let source = container
        .sources
        .iter()
        .find(|source| source.id == seismograph_rallocator::source::ID)
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "recording has no native allocator source"))?;
    let snapshot = seismograph_rallocator::decode(&source.data).map_err(std::io::Error::other)?;
    assert_eq!(
        u128::from(snapshot.global.reserved_bytes) + u128::from(snapshot.global.pagemap_reserved_bytes),
        274_894_684_160
    );
    assert_eq!(snapshot.global.ranges.observed_bytes(), 6_373_376);
    std::fs::write(
        directory.join("actual-memory.txt"),
        render(Some(&snapshot), Navigation::default(), 140, 38),
    )?;
    std::fs::write(
        directory.join("actual-global.txt"),
        render(
            Some(&snapshot),
            Navigation {
                root: 1,
                depth: Depth::Detail,
                ..Default::default()
            },
            140,
            38,
        ),
    )?;
    for (index, owner) in snapshot.owners.iter().enumerate() {
        let nav = Navigation {
            root: index + 2,
            selected_owner: Some(owner.id),
            ..Default::default()
        };
        std::fs::write(
            directory.join(format!("actual-owner-{}.txt", index + 1)),
            render(Some(&snapshot), nav, 140, 38),
        )?;
        if let Some(observation) = &owner.observation {
            let class = observation
                .classes
                .iter()
                .enumerate()
                .max_by_key(|(_, class)| class.observed_slabs)
                .map(|(index, _)| index)
                .ok_or_else(|| std::io::Error::other("observation has no classes"))?;
            let classes = Navigation {
                depth: Depth::Classes,
                class,
                ..nav
            };
            std::fs::write(
                directory.join(format!("actual-small-{}.txt", index + 1)),
                render(Some(&snapshot), classes, 140, 38),
            )?;
            let detail = Navigation {
                depth: Depth::Class,
                ..classes
            };
            std::fs::write(
                directory.join(format!("actual-class-{}.txt", index + 1)),
                render(Some(&snapshot), detail, 140, 38),
            )?;
            std::fs::write(
                directory.join(format!("actual-class-narrow-{}.txt", index + 1)),
                render(Some(&snapshot), detail, 80, 24),
            )?;
            std::fs::write(
                directory.join(format!("actual-class-narrow-end-{}.txt", index + 1)),
                render(
                    Some(&snapshot),
                    Navigation {
                        scroll: usize::MAX,
                        ..detail
                    },
                    80,
                    24,
                ),
            )?;
        }
    }
    Ok(())
}
