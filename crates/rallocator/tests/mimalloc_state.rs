// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Cooperative mimalloc state through the installed global allocator.

use std::alloc::Layout;
use std::ptr::NonNull;
use std::sync::mpsc;

use seismograph::recorder::{Configuration, Policy, without_recording};
use seismograph_rallocator::mimalloc::{ObservationSource, State};
use seismograph_rallocator::snapshot::{self, BackendState, Header};

rallocator::rallocator!(backend = mimalloc);

struct Allocation {
    pointer: NonNull<u8>,
    layout: Layout,
}

impl Allocation {
    fn new(layout: Layout) -> Self {
        // SAFETY: The caller supplies a valid nonzero Layout.
        let pointer = unsafe { std::alloc::alloc(layout) };
        let Some(pointer) = NonNull::new(pointer) else {
            std::alloc::handle_alloc_error(layout)
        };
        Self { pointer, layout }
    }

    fn transfer(self) -> usize {
        let address = self.pointer.as_ptr().expose_provenance();
        std::mem::forget(self);
        address
    }
}

impl Drop for Allocation {
    fn drop(&mut self) {
        // SAFETY: This fixture owns the installed backend's live allocation
        // and retires it exactly once with its original layout.
        unsafe { std::alloc::dealloc(self.pointer.as_ptr(), self.layout) };
    }
}

#[expect(clippy::unwrap_used, clippy::panic, reason = "The fixture requires a valid mimalloc source")]
fn capture() -> (Header, State) {
    let captured = seismograph::snapshot(seismograph::snapshot::SnapshotOptions::default()).unwrap();
    let decoded = seismograph::snapshot::decode(captured.as_bytes()).unwrap();
    let source = decoded
        .sources
        .iter()
        .find(|source| source.id == seismograph_rallocator::source::MIMALLOC_ID)
        .unwrap();
    assert_eq!(source.schema_version, snapshot::SCHEMA_VERSION);
    let decoded = snapshot::decode(&source.data).unwrap();
    let BackendState::Mimalloc(state) = decoded.backend else {
        panic!("mimalloc source must carry mimalloc state")
    };
    (decoded.header, state)
}

enum Command {
    Emit,
    EmitSuppressed,
    Transfer,
    Quit,
}

enum Response {
    Ready,
    Published(u64),
    Transferred(usize),
}

#[expect(clippy::unwrap_used, reason = "The worker must complete the scripted test protocol")]
fn worker(layout: Layout, os_layout: Layout, commands: mpsc::Receiver<Command>, responses: &mpsc::Sender<Response>) {
    let (_retained, _os) = without_recording(|| {
        // Warm the native NULL-slot entry before arranging the fixed class.
        drop(Allocation::new(Layout::from_size_align(64, 16).unwrap()));
        (Allocation::new(layout), Allocation::new(os_layout))
    });
    without_recording(|| responses.send(Response::Ready)).unwrap();
    while let Ok(command) = without_recording(|| commands.recv()) {
        match command {
            Command::Emit | Command::EmitSuppressed => {
                let operation = || drop(Allocation::new(Layout::from_size_align(64, 16).unwrap()));
                if matches!(command, Command::EmitSuppressed) {
                    without_recording(operation);
                } else {
                    operation();
                }
                without_recording(|| responses.send(Response::Published(seismograph::recorder::thread_id().get()))).unwrap();
            }
            Command::Transfer => {
                let address = Allocation::new(layout).transfer();
                without_recording(|| responses.send(Response::Transferred(address))).unwrap();
            }
            Command::Quit => break,
        }
    }
}

#[test]
fn owner_publication_is_cooperative_and_survives_thread_cleanup() {
    // The controller remains quiet while its worker exercises real recording.
    without_recording(|| {
        seismograph::recorder(Configuration::default());
        snapshot::set_publication_enabled(true);
        std::thread::scope(|scope| {
            let (commands, receive_commands) = mpsc::channel();
            let (responses, receive_responses) = mpsc::channel();
            let layout = Layout::from_size_align(512, 16).unwrap();
            let os_layout = Layout::from_size_align(131_073, 131_072).unwrap();
            let worker = scope.spawn(move || worker(layout, os_layout, receive_commands, &responses));
            assert!(matches!(receive_responses.recv().unwrap(), Response::Ready));
            let (_, before) = capture();
            assert!(before.global.reserved_bytes > 0);
            assert!(before.global.os_allocations > 0);
            assert!(before.global.os_committed_bytes >= os_layout.size() as u64);

            seismograph::recorder(Configuration {
                allocations: Policy::all(false),
                ..Default::default()
            });
            commands.send(Command::Emit).unwrap();
            let Response::Published(thread_id) = receive_responses.recv().unwrap() else {
                panic!("expected recorded operation")
            };
            let (_, published) = capture();
            let owner = published
                .owners
                .iter()
                .find(|owner| owner.observation.is_some_and(|value| value.thread_id == thread_id))
                .unwrap();
            let id = owner.id;
            let first = owner.observation.unwrap();
            assert_eq!(
                before.owners.iter().find(|owner| owner.id == id).unwrap().source,
                ObservationSource::Unobserved
            );
            assert_eq!(owner.source, ObservationSource::Published);
            assert!(
                first.classes.iter().any(|class| class.block_bytes == 512 && class.used_blocks >= 1),
                "retained class absent from the owner copy: {:?}",
                first.classes.iter().filter(|class| class.observed_pages != 0).collect::<Vec<_>>()
            );

            snapshot::set_publication_enabled(false);
            commands.send(Command::Emit).unwrap();
            assert!(matches!(receive_responses.recv().unwrap(), Response::Published(_)));
            let (disabled_header, disabled) = capture();
            assert!(!disabled_header.publication_enabled);
            assert_eq!(
                disabled.owners.iter().find(|owner| owner.id == id).unwrap().observation,
                Some(first)
            );
            snapshot::set_publication_enabled(true);

            commands.send(Command::EmitSuppressed).unwrap();
            assert!(matches!(receive_responses.recv().unwrap(), Response::Published(_)));
            let (_, suppressed) = capture();
            assert_eq!(
                suppressed.owners.iter().find(|owner| owner.id == id).unwrap().observation,
                Some(first)
            );

            commands.send(Command::Transfer).unwrap();
            let Response::Transferred(address) = receive_responses.recv().unwrap() else {
                panic!("expected allocation transfer")
            };
            // SAFETY: The worker transfers the live block once, with its exact
            // original layout. The controller does not access its payload.
            unsafe { std::alloc::dealloc(std::ptr::with_exposed_provenance_mut(address), layout) };
            snapshot::request_observation();
            commands.send(Command::Emit).unwrap();
            assert!(matches!(receive_responses.recv().unwrap(), Response::Published(_)));
            let (header, updated) = capture();
            let owner = updated.owners.iter().find(|owner| owner.id == id).unwrap();
            let observation = owner.observation.unwrap();
            assert!(observation.round > first.round);
            assert_eq!(observation.session_id, header.session_id);
            assert!(
                observation
                    .classes
                    .iter()
                    .any(|class| class.block_bytes == 512 && class.used_blocks >= 2)
            );
            assert!(observation.pages_complete && observation.free_lists_complete);

            seismograph::recorder(Configuration::default());
            commands.send(Command::Emit).unwrap();
            assert!(matches!(receive_responses.recv().unwrap(), Response::Published(_)));
            let (_, stopped) = capture();
            assert_eq!(
                stopped.owners.iter().find(|owner| owner.id == id).unwrap().observation,
                Some(observation)
            );
            commands.send(Command::Quit).unwrap();
            worker.join().unwrap();

            let (header, exited) = capture();
            let owner = exited.owners.iter().find(|owner| owner.id == id).unwrap();
            assert!(!owner.active);
            assert_eq!(owner.observation, Some(observation));
            assert_eq!(owner.freshness(&header), seismograph_rallocator::native::Freshness::PreviousLease);
            assert!(exited.global.os_committed_bytes < before.global.os_committed_bytes);
        });
    });
}
