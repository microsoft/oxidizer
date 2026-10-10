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
fn captured_native_inventory_acceptance_screens() -> std::io::Result<()> {
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("native-ui")
        .join("captured-inventory");
    std::fs::create_dir_all(&directory)?;
    // Native source extracted from the actual recording; unrelated runtime events are not included.
    let input = include_bytes!("../../../../tests/fixtures/captured-native-v4.bin");
    let snapshot = seismograph_rallocator::decode(input).map_err(std::io::Error::other)?;
    assert_eq!(
        u128::from(snapshot.global.reserved_bytes) + u128::from(snapshot.global.pagemap_reserved_bytes),
        274_894_684_160
    );
    assert_eq!(snapshot.global.ranges.observed_bytes(), 6_373_376);
    write_inventory_screens(&snapshot, &directory)?;
    for (file, expected) in [
        ("actual-memory.txt", &["OS reserved 256.02 GiB", "Global cached 6.08 MiB"][..]),
        ("actual-global.txt", &["Cached capacity 6.08 MiB"][..]),
        (
            "actual-owner-1.txt",
            &["Owner 1 [Observed]", "Small slabs 656.00 KiB", "Large ranges 640.00 KiB"][..],
        ),
        (
            "actual-class-1.txt",
            &[
                "C43 [Observed]",
                "Object size 64.00 KiB",
                "Slab size 256.00 KiB",
                "Slots / slab 4",
                "Slabs 2",
            ][..],
        ),
    ] {
        let screen = std::fs::read_to_string(directory.join(file))?;
        let normalized = screen.split_whitespace().collect::<Vec<_>>().join(" ");
        for evidence in expected {
            assert!(normalized.contains(evidence), "{file}: missing {evidence}\n{screen}");
        }
    }
    Ok(())
}

#[test]
fn encoded_inventory_acceptance_screens_preserve_owner_and_class_evidence() {
    static SOURCE: seismograph::snapshot::Source = seismograph::snapshot::Source::new(
        seismograph_rallocator::source::ID,
        "native-ui-roundtrip",
        seismograph_rallocator::source::SCHEMA_VERSION,
        capture_fixture,
    );
    seismograph::snapshot::register_source(&SOURCE);
    let recording = seismograph::snapshot(seismograph::snapshot::SnapshotOptions::default()).unwrap();
    let container = seismograph::snapshot::decode(recording.as_bytes()).unwrap();
    let source = container
        .sources
        .iter()
        .find(|source| source.id == seismograph_rallocator::source::ID)
        .unwrap();
    let snapshot = seismograph_rallocator::decode(&source.data).unwrap();
    let fixture = crate::native_view::fixture::snapshot();
    assert_eq!(snapshot.owners.len(), fixture.owners.len());
    assert_eq!(snapshot.global.ranges.observed_bytes(), fixture.global.ranges.observed_bytes());
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("native-ui")
        .join("roundtrip-fixture");
    std::fs::create_dir_all(&directory).unwrap();
    write_inventory_screens(&snapshot, &directory).unwrap();
    for (index, owner) in snapshot.owners.iter().enumerate() {
        let output = std::fs::read_to_string(directory.join(format!("actual-owner-{}.txt", index + 1))).unwrap();
        assert_eq!(owner.id, fixture.owners[index].id);
        assert!(output.contains(&format!("Owner {}", index + 1)), "{output}");
        if owner.observation.is_some() {
            for suffix in ["", "-narrow", "-narrow-end"] {
                let output = std::fs::read_to_string(directory.join(format!("actual-class{suffix}-{}.txt", index + 1))).unwrap();
                assert!(!output.contains("NaN"), "{output}");
                assert_eq!(output.lines().count(), if suffix.is_empty() { 38 } else { 24 });
            }
        }
    }
}

#[test]
fn inventory_screen_exports_propagate_each_output_file_error() {
    let snapshot = crate::native_view::fixture::snapshot();
    for filename in [
        "actual-memory.txt",
        "actual-global.txt",
        "actual-owner-1.txt",
        "actual-small-1.txt",
        "actual-class-1.txt",
        "actual-class-narrow-1.txt",
        "actual-class-narrow-end-1.txt",
    ] {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("native-ui")
            .join(format!("write-error-{filename}"));
        std::fs::create_dir_all(directory.join(filename)).unwrap();
        assert!(write_inventory_screens(&snapshot, &directory).is_err(), "{filename}");
        assert!(directory.join(filename).is_dir());
        std::fs::remove_dir_all(directory).unwrap();
    }
}

fn capture_fixture(_context: seismograph::snapshot::SnapshotContext<'_>) -> Result<seismograph::snapshot::SourceData, seismograph::Error> {
    let snapshot = crate::native_view::fixture::snapshot();
    let mut data = seismograph::snapshot::SourceData::zeroed(seismograph_rallocator::encoded_len(&snapshot).unwrap())?;
    seismograph_rallocator::encode(&snapshot, data.as_mut_bytes()).unwrap();
    Ok(data)
}

fn write_inventory_screens(snapshot: &Snapshot, directory: &std::path::Path) -> std::io::Result<()> {
    std::fs::write(
        directory.join("actual-memory.txt"),
        render(Some(snapshot), Navigation::default(), 140, 38),
    )?;
    std::fs::write(
        directory.join("actual-global.txt"),
        render(
            Some(snapshot),
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
            render(Some(snapshot), nav, 140, 38),
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
                render(Some(snapshot), classes, 140, 38),
            )?;
            let detail = Navigation {
                depth: Depth::Class,
                ..classes
            };
            std::fs::write(
                directory.join(format!("actual-class-{}.txt", index + 1)),
                render(Some(snapshot), detail, 140, 38),
            )?;
            std::fs::write(
                directory.join(format!("actual-class-narrow-{}.txt", index + 1)),
                render(Some(snapshot), detail, 80, 24),
            )?;
            std::fs::write(
                directory.join(format!("actual-class-narrow-end-{}.txt", index + 1)),
                render(
                    Some(snapshot),
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
