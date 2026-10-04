// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Native inventory and collaborative observation end-to-end regression.

use std::alloc::{GlobalAlloc, Layout};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

use rallocator::Rallocator;
use seismograph::recorder::{Configuration, RecordingPolicy, SuppressionGuard};
use seismograph_rallocator::native::{ObservationSource, Snapshot};

rallocator::rallocator!();

static RECORDING: Mutex<()> = Mutex::new(());

fn capture() -> Snapshot {
    let snapshot = seismograph::snapshot(seismograph::snapshot::SnapshotOptions::default()).expect("capture native snapshot");
    let _suppression = SuppressionGuard::enter();
    let decoded = seismograph::snapshot::decode(snapshot.as_bytes()).expect("decode Seismograph snapshot");
    let source = decoded
        .sources
        .iter()
        .find(|source| source.id == seismograph_rallocator::source::ID)
        .expect("native allocator source is registered");
    assert_eq!(source.schema_version, 3);
    seismograph_rallocator::decode(&source.data).expect("decode native allocator source")
}

fn wait_for(progress: &AtomicU8, value: u8) {
    while progress.load(Ordering::Acquire) < value {
        std::hint::spin_loop();
    }
}

#[test]
fn owners_created_off_remain_visible_and_only_participants_publish() {
    let _recording = RECORDING.lock().unwrap();
    seismograph::recorder(Configuration::default());
    seismograph_rallocator::native::set_publication_enabled(true);
    let command = Arc::new(AtomicU8::new(0));
    let progress = Arc::new(AtomicU8::new(0));
    let worker_command = Arc::clone(&command);
    let worker_progress = Arc::clone(&progress);
    let worker = std::thread::spawn(move || {
        let layout = Layout::from_size_align(17023, 128).unwrap();
        // SAFETY: The nonzero allocation is checked and freed once with its layout.
        let pointer = unsafe { Rallocator.alloc(layout) };
        assert!(!pointer.is_null());
        worker_progress.store(1, Ordering::Release);
        let mut handled = 0;
        loop {
            let requested = worker_command.load(Ordering::Acquire);
            if requested == 3 {
                break;
            }
            if requested == handled {
                std::hint::spin_loop();
                continue;
            }
            let active_layout = Layout::from_size_align(513, 16).unwrap();
            // SAFETY: Each checked allocation is freed once using its original layout.
            let active = unsafe { Rallocator.alloc(active_layout) };
            assert!(!active.is_null());
            // SAFETY: The still-live allocation belongs to this allocator and layout.
            unsafe { Rallocator.dealloc(active, active_layout) };
            handled = requested;
            worker_progress.store(requested + 1, Ordering::Release);
        }
        // SAFETY: The original allocation remains live throughout the worker.
        unsafe { Rallocator.dealloc(pointer, layout) };
    });
    wait_for(&progress, 1);
    let before = capture();
    assert!(before.owners_complete);
    assert!(before.owners.iter().any(|owner| owner.leased && owner.observation.is_none()));

    seismograph::recorder(Configuration {
        allocations: RecordingPolicy::all(false),
        ..Configuration::default()
    });
    let quiet = capture();
    // Thread names and ownership are learned only from contributors, not invented for quiet leases.
    assert!(
        quiet
            .owners
            .iter()
            .any(|owner| { owner.leased && owner.source == ObservationSource::Unobserved && owner.observation.is_none() })
    );
    command.store(1, Ordering::Release);
    wait_for(&progress, 2);
    let active = capture();
    let contributed = active
        .owners
        .iter()
        .find(|owner| {
            owner.leased
                && owner.source == ObservationSource::Published
                && owner.observation.is_some_and(|state| {
                    state.generation == owner.generation && state.session_id == active.session_id && state.round == active.round
                })
        })
        .unwrap();
    let observation = contributed.observation.unwrap();
    assert!(observation.thread_id != 0);
    assert!(observation.classes.iter().any(|class| class.observed_slabs != 0));
    assert!(
        observation
            .classes
            .windows(2)
            .all(|pair| pair[0].object_bytes < pair[1].object_bytes)
    );
    let owner_id = contributed.id;

    // Publication disabled still permits allocation/free events and inventory captures.
    seismograph_rallocator::native::set_publication_enabled(false);
    command.store(2, Ordering::Release);
    wait_for(&progress, 3);
    let disabled = capture();
    let old = disabled.owners.iter().find(|owner| owner.id == owner_id).unwrap();
    assert_eq!(old.observation.unwrap().round, observation.round);
    assert!(!disabled.publication_enabled);

    command.store(3, Ordering::Release);
    worker.join().unwrap();
    seismograph::recorder(Configuration::default());
    let returned = capture();
    let owner = returned.owners.iter().find(|owner| owner.id == owner_id).unwrap();
    assert!(!owner.leased);
    assert_eq!(owner.source, ObservationSource::IdleInspection);
    assert_eq!(owner.observation.unwrap().generation, owner.generation);
    assert!(owner.generation > observation.generation);
    seismograph_rallocator::native::set_publication_enabled(true);
}

#[test]
fn concurrent_collection_reads_published_copies_not_mutating_cores() {
    let _recording = RECORDING.lock().unwrap();
    seismograph_rallocator::native::set_publication_enabled(true);
    seismograph::recorder(Configuration {
        allocations: RecordingPolicy::all(false),
        ..Configuration::default()
    });
    let stop = Arc::new(AtomicU8::new(0));
    std::thread::scope(|scope| {
        for lane in 0..4 {
            let stop = Arc::clone(&stop);
            scope.spawn(move || {
                let mut iteration = 0;
                while stop.load(Ordering::Acquire) == 0 {
                    let layout = Layout::from_size_align(17 + ((iteration + lane) % 8) * 513, 16).unwrap();
                    // SAFETY: The nonzero allocation is checked before payload access and release.
                    let pointer = unsafe { Rallocator.alloc(layout) };
                    assert!(!pointer.is_null());
                    // SAFETY: The whole requested region remains exclusively owned by this worker.
                    unsafe { pointer.write_bytes(37, layout.size()) };
                    // SAFETY: The worker releases its live allocation exactly once with the original layout.
                    unsafe { Rallocator.dealloc(pointer, layout) };
                    iteration += 1;
                }
            });
        }
        for _ in 0..20 {
            let state = capture();
            assert!(state.owners_complete);
            assert_eq!(state.owner_count, state.owners.len() as u64);
            for owner in &state.owners {
                assert_eq!(owner.leased, owner.generation & 1 != 0);
                if let Some(observation) = owner.observation {
                    assert!(observation.captured_nanos <= state.captured_nanos);
                    assert!(
                        observation
                            .classes
                            .iter()
                            .all(|class| { class.object_bytes > 0 && class.slab_bytes >= class.object_bytes && class.capacity > 0 })
                    );
                }
            }
        }
        stop.store(1, Ordering::Release);
    });
    seismograph::recorder(Configuration::default());
}
