// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Exercises recorder payloads and native publication without timestamp oracles.

use std::alloc::{GlobalAlloc, Layout};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};

use GLOBAL as Rallocator;
use seismograph::recorder::event::{Event, EventKind};
use seismograph::recorder::{Configuration, Policy, thread_id, without_recording};
use seismograph_rallocator::native::{ObservationSource, Snapshot};

rallocator::rallocator!();

fn capture() -> (Snapshot, Vec<Event>) {
    let round = seismograph_rallocator::native::observation_round();
    let recording = seismograph::snapshot(seismograph::snapshot::SnapshotOptions::default())
        .expect("the registered native allocator source must capture successfully");
    assert_eq!(seismograph_rallocator::native::observation_round(), round + 1);
    without_recording(|| {
        let decoded = seismograph::snapshot::decode(recording.as_bytes()).expect("the recorder must produce a valid container");
        let source = decoded
            .sources
            .iter()
            .find(|source| source.id == seismograph_rallocator::source::ID)
            .expect("the global allocator registers its native source before capture");
        assert_eq!(source.schema_version, seismograph_rallocator::source::SCHEMA_VERSION);
        let state = seismograph_rallocator::decode(&source.data).expect("the native source must encode a valid inventory");
        assert_eq!(state.round, round);
        assert!(state.owners_complete);
        assert_eq!(state.owner_count, state.owners.len() as u64);
        let events = decoded.events.events;
        assert!(
            events
                .iter()
                .filter_map(Event::allocation)
                .all(|allocation| allocation.address.get() != 0)
        );
        (state, events)
    })
}

fn enable() {
    seismograph::recorder(Configuration {
        allocations: Policy::all(false),
        ..Configuration::default()
    });
}

fn recorded_worker(commands: &Receiver<()>, ready: &SyncSender<(u64, usize)>) {
    let held_layout = Layout::from_size_align(513, 16).expect("the fixture layout is valid");
    // SAFETY: The nonzero layout is valid; the checked allocation is retained until worker shutdown.
    let held = unsafe { Rallocator.alloc(held_layout) };
    assert!(!held.is_null());
    ready.send((0, 0)).expect("the collector waits for worker initialization");

    commands.recv().expect("the collector requests the first recorded allocation");
    let old_layout = Layout::from_size_align(33, 16).expect("the fixture layout is valid");
    // SAFETY: The valid allocation is checked before initialization and later resized once.
    let original = unsafe { Rallocator.alloc(old_layout) };
    assert!(!original.is_null());
    // SAFETY: The worker uniquely owns all 33 writable requested bytes.
    unsafe { original.write_bytes(0x61, 33) };
    ready
        .send((thread_id().get(), original.addr()))
        .expect("the collector waits for the recorded allocation");

    commands.recv().expect("the collector requests resize after restarting recording");
    // SAFETY: The original remains live with its exact old layout; the new size is valid.
    let replacement = unsafe { Rallocator.realloc(original, old_layout, 4097) };
    assert!(!replacement.is_null());
    assert_ne!(replacement, original);
    // SAFETY: Successful resizing preserves the initialized 33-byte prefix.
    assert_eq!(unsafe { std::slice::from_raw_parts(replacement, 33) }, [0x61; 33]);
    ready
        .send((thread_id().get(), replacement.addr()))
        .expect("the collector waits for resize completion");

    commands.recv().expect("the collector requests the recorded free");
    let replacement_layout = Layout::from_size_align(4097, 16).expect("the resized layout is valid");
    // SAFETY: The replacement is live with this exact resized layout and is returned once.
    unsafe { Rallocator.dealloc(replacement, replacement_layout) };
    ready
        .send((thread_id().get(), replacement.addr()))
        .expect("the collector waits for the recorded free");

    commands
        .recv()
        .expect("the collector requests an allocation while publication is disabled");
    let rejected_layout = Layout::from_size_align(1usize << 47, 16).expect("the oversized fixture remains a valid Layout");
    // SAFETY: The layout is valid; this request exceeds the allocator's checked native limit.
    assert!(unsafe { Rallocator.alloc(rejected_layout) }.is_null());
    let zeroed_layout = Layout::from_size_align(73, 64).expect("the zeroed fixture layout is valid");
    // SAFETY: The valid nonzero allocation is checked and freed once below.
    let zeroed = unsafe { Rallocator.alloc_zeroed(zeroed_layout) };
    assert!(!zeroed.is_null());
    // SAFETY: alloc_zeroed initializes every requested byte and the allocation remains live.
    assert_eq!(unsafe { std::slice::from_raw_parts(zeroed, 73) }, [0; 73]);
    ready
        .send((thread_id().get(), zeroed.addr()))
        .expect("the collector waits for the zeroed allocation");

    commands.recv().expect("the collector stops recording before worker cleanup");
    // SAFETY: The worker still owns the zeroed allocation with its original exact layout.
    unsafe { Rallocator.dealloc(zeroed, zeroed_layout) };
    // SAFETY: The original held allocation remains live and is returned once on the same thread.
    unsafe { Rallocator.dealloc(held, held_layout) };
}

fn assert_resize_events(events: &[Event], original: usize, replacement: usize) {
    let records = events
        .iter()
        .filter_map(Event::reallocation)
        .filter(|resize| resize.old_address.get() == original as u64)
        .collect::<Vec<_>>();
    assert_eq!(records.len(), 1);
    let resize = records[0];
    assert_eq!(resize.new_address.get(), replacement as u64);
    assert_eq!((resize.old_size, resize.new_size, resize.alignment), (33, 4097, 16));
    assert_eq!(resize.allocation_id.get(), original as u64);
    assert_eq!(resize.heap_id.get(), 0);
    assert!(!events.iter().any(|event| {
        event.allocation().is_some_and(|allocation| {
            (event.kind == EventKind::Deallocation && allocation.address.get() == original as u64)
                || (event.kind == EventKind::Allocation && allocation.address.get() == replacement as u64)
        })
    }));
}

fn assert_free_event(events: &[Event], replacement: usize) {
    let records = events
        .iter()
        .filter_map(|event| event.allocation().map(|allocation| (event.kind, allocation)))
        .filter(|(_, allocation)| allocation.address.get() == replacement as u64)
        .collect::<Vec<_>>();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].0, EventKind::Deallocation);
    let allocation = records[0].1;
    assert_eq!((allocation.size, allocation.alignment), (4097, 16));
    assert_eq!(allocation.allocation_id.get(), replacement as u64);
    assert_eq!(allocation.heap_id.get(), 0);
    assert_eq!(allocation.event_thread_id.get(), 0);
    assert!(!allocation.freed_after_heap_release);
}

#[test]
fn gamma_round2_recorded_resize_free_and_restart_publish_current_non_time_state() {
    let (command, commands) = sync_channel::<()>(0);
    let (ready, responses) = sync_channel::<(u64, usize)>(0);
    let worker = std::thread::spawn(move || recorded_worker(&commands, &ready));
    responses
        .recv()
        .expect("the worker initializes its native owner while recording is disabled");
    let (initial, _) = capture();
    assert_eq!(initial.session_id, 0);
    enable();
    command.send(()).expect("the worker is waiting for its first allocation");
    let (_, original) = responses.recv().expect("the worker reports its first allocation");
    seismograph::recorder(Configuration::default());
    enable();
    command.send(()).expect("the worker is waiting for resize");
    let (thread, replacement) = responses.recv().expect("the worker reports its replacement");
    let (resized, events) = capture();
    let owner = resized
        .owners
        .iter()
        .find(|owner| owner.leased && owner.observation.is_some_and(|state| state.thread_id == thread))
        .expect("an accepted resize publishes the contributing worker's native owner");
    assert_eq!(owner.source, ObservationSource::Published);
    assert_eq!(owner.generation & 1, 1);
    let state = owner.observation.expect("the contributing owner has a published observation");
    assert_eq!(resized.session_id, 0);
    assert_eq!(state.session_id, resized.session_id);
    assert_eq!(state.generation, owner.generation);
    assert_eq!(state.round, resized.round);
    let owner_id = owner.id;
    let generation = owner.generation;
    assert_resize_events(&events, original, replacement);

    command.send(()).expect("the worker is waiting for its recorded free");
    responses.recv().expect("the worker reports that the replacement has been freed");
    let (freed, events) = capture();
    let owner = freed
        .owners
        .iter()
        .find(|owner| owner.id == owner_id)
        .expect("the worker retains the same native owner");
    let state = owner.observation.expect("an accepted free publishes a current observation");
    assert_eq!(state.round, freed.round);
    assert_eq!(
        (state.generation, state.session_id, state.thread_id),
        (generation, freed.session_id, thread)
    );
    assert_free_event(&events, replacement);
    assert_resize_events(&events, original, replacement);

    seismograph_rallocator::native::set_publication_enabled(false);
    command
        .send(())
        .expect("the worker is waiting for the publication-disabled allocation");
    let (_, zeroed) = responses.recv().expect("the worker reports its zeroed allocation");
    let (disabled, events) = capture();
    assert!(!disabled.publication_enabled);
    let retained = disabled
        .owners
        .iter()
        .find(|owner| owner.id == owner_id)
        .expect("disabling publication preserves inventory");
    assert_eq!(
        retained.observation.expect("the last publication remains readable").round,
        state.round
    );
    assert!(events.iter().any(|event| {
        event.kind == EventKind::Allocation
            && event
                .allocation()
                .is_some_and(|allocation| allocation.address.get() == zeroed as u64 && allocation.size == 73 && allocation.alignment == 64)
    }));

    seismograph::recorder(Configuration::default());
    command.send(()).expect("the worker is waiting for shutdown");
    worker
        .join()
        .expect("the deterministic worker must complete without allocation or payload failures");
    let (returned, _) = capture();
    assert_eq!(returned.session_id, resized.session_id);
    let owner = returned
        .owners
        .iter()
        .find(|owner| owner.id == owner_id)
        .expect("returned owners remain in native inventory");
    assert!(!owner.leased);
    assert_eq!(owner.source, ObservationSource::IdleInspection);
    assert!(owner.generation > generation);
    assert_eq!(owner.generation & 1, 0);
    assert_eq!(
        owner.observation.expect("the collector inspects the returned owner").generation,
        owner.generation
    );
    seismograph_rallocator::native::set_publication_enabled(true);
}
