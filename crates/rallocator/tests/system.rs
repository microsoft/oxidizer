// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The default system backend records allocation events like the native backends.

use std::alloc::{GlobalAlloc, Layout};
use std::ptr::NonNull;

use seismograph::recorder::event::{Event, EventKind};
use seismograph::recorder::{Configuration, Policy, without_recording};

rallocator::rallocator!();

const _: &rallocator::Rallocator<rallocator::backend::System> = &GLOBAL;
const _: &rallocator::Rallocator = &GLOBAL;

#[test]
fn default_system_backend_records_allocation_resize_and_free_events() {
    seismograph::recorder(Configuration {
        allocations: Policy::all(false),
        ..Default::default()
    });
    let layout = Layout::from_size_align(83, 64).unwrap();
    let grown = Layout::from_size_align(9001, layout.align()).unwrap();
    // SAFETY: The layout is valid and nonzero.
    let old = NonNull::new(unsafe { GLOBAL.alloc_zeroed(layout) }).unwrap();
    let old_address = old.addr().get() as u64;
    // SAFETY: This test exclusively owns all layout.size() initialized bytes.
    let bytes = unsafe { std::slice::from_raw_parts_mut(old.as_ptr(), layout.size()) };
    assert_eq!(bytes, &[0; 83]);
    bytes.fill(0xb7);
    // SAFETY: The block is live for its layout and the new size is valid and nonzero.
    let new = NonNull::new(unsafe { GLOBAL.realloc(old.as_ptr(), layout, grown.size()) }).unwrap();
    let new_address = new.addr().get() as u64;
    // SAFETY: A successful resize preserves all 83 initialized bytes.
    assert_eq!(unsafe { std::slice::from_raw_parts(new.as_ptr(), layout.size()) }, [0xb7; 83]);
    // SAFETY: Retire the resized block exactly once with its new layout.
    unsafe { GLOBAL.dealloc(new.as_ptr(), grown) };

    without_recording(|| {
        let capture = seismograph::snapshot(seismograph::snapshot::SnapshotOptions::default()).unwrap();
        let decoded = seismograph::snapshot::decode(capture.as_bytes()).unwrap();
        let events = &decoded.events.events;
        let blocks: Vec<_> = events
            .iter()
            .filter_map(|event| event.allocation().map(|allocation| (event.kind, allocation)))
            .filter(|(_, allocation)| [old_address, new_address].contains(&allocation.address.get()))
            .map(|(kind, allocation)| (kind, allocation.address.get(), allocation.size, allocation.alignment))
            .collect();
        assert_eq!(
            blocks,
            [
                (EventKind::Allocation, old_address, 83, 64),
                (EventKind::Deallocation, new_address, 9001, 64)
            ]
        );
        let resizes: Vec<_> = events
            .iter()
            .filter_map(Event::reallocation)
            .filter(|resize| resize.old_address.get() == old_address)
            .map(|resize| (resize.new_address.get(), resize.old_size, resize.new_size, resize.alignment))
            .collect();
        assert_eq!(resizes, [(new_address, 83, 9001, 64)]);
    });
}
