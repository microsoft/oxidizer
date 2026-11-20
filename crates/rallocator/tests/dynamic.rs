// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Exercise the installed dynamic allocator across threads and through the recorder.

use std::alloc::{GlobalAlloc, Layout};
use std::ptr::NonNull;

use seismograph::recorder::event::Event;
use seismograph::recorder::{Configuration, Policy, without_recording};

rallocator::rallocator!(backend = dynamic);

#[test]
fn installed_allocator_transfers_blocks_and_records_only_successful_resizes() {
    let mut buffer = std::thread::spawn(|| vec![0x5a_u8; 113]).join().unwrap();
    buffer.reserve(8192);
    assert_eq!(buffer, [0x5a; 113]);
    drop(buffer);

    seismograph::recorder(Configuration {
        allocations: Policy::all(false),
        ..Default::default()
    });
    let layout = Layout::from_size_align(83, 64).unwrap();
    let grown = Layout::from_size_align(9001, layout.align()).unwrap();
    // SAFETY: The layout is valid and nonzero.
    let old = NonNull::new(unsafe { GLOBAL.alloc_zeroed(layout) }).unwrap();
    let old_address = old.addr().get();
    assert_eq!(old_address % layout.align(), 0);
    // SAFETY: This test exclusively owns all layout.size() initialized bytes.
    let bytes = unsafe { std::slice::from_raw_parts_mut(old.as_ptr(), layout.size()) };
    assert_eq!(bytes, &[0; 83]);
    bytes.fill(0xb7);
    // SAFETY: This valid nonzero size exceeds the supported user address space.
    assert!(unsafe { GLOBAL.realloc(old.as_ptr(), layout, 1 << 62) }.is_null());
    // SAFETY: Failure leaves the original block and its layout live.
    let retained = NonNull::new(unsafe { GLOBAL.realloc(old.as_ptr(), layout, layout.size()) }).unwrap();
    assert_eq!(retained.addr().get(), old_address);
    // SAFETY: The successful in-place resize transferred ownership to retained.
    let new = NonNull::new(unsafe { GLOBAL.realloc(retained.as_ptr(), layout, grown.size()) }).unwrap();
    let new_address = new.addr().get();
    assert_eq!(new_address % grown.align(), 0);
    // SAFETY: A successful resize preserves all 83 initialized bytes.
    assert_eq!(unsafe { std::slice::from_raw_parts(new.as_ptr(), layout.size()) }, [0xb7; 83]);
    // SAFETY: Retire the resized block exactly once with its new layout.
    unsafe { GLOBAL.dealloc(new.as_ptr(), grown) };

    without_recording(|| {
        let capture = seismograph::snapshot(seismograph::snapshot::SnapshotOptions::default()).unwrap();
        let decoded = seismograph::snapshot::decode(capture.as_bytes()).unwrap();
        let resizes: Vec<_> = decoded
            .events
            .events
            .iter()
            .filter_map(Event::reallocation)
            .filter(|resize| resize.old_address.get() == old_address as u64)
            .map(|resize| (resize.new_address.get(), resize.old_size, resize.new_size, resize.alignment))
            .collect();
        assert_eq!(resizes, [(new_address as u64, 83, 9001, 64)]);
    });
}
