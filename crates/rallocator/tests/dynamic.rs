// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Launch the same executable with each backend and observe which one owns its allocations.

use std::alloc::{GlobalAlloc, Layout};
use std::process::Command;
use std::ptr::NonNull;

use seismograph::recorder::event::Event;
use seismograph::recorder::{Configuration, Policy, without_recording};
use seismograph_rallocator::source;

rallocator::rallocator!(backend = dynamic);

#[test]
fn launch_selects_the_backend_for_all_threads() {
    const EXPECTED: &str = "RALLOCATOR_TEST_EXPECTED_BACKEND";
    if let Ok(expected) = std::env::var(EXPECTED) {
        exercise(&expected);
        return;
    }

    for (setting, expected, warn) in [
        (None, "system", false),
        (Some(""), "system", false),
        (Some("snmalloc"), "snmalloc", false),
        (Some("mimalloc"), "mimalloc", false),
        (Some("system"), "system", false),
        (Some("mimalloc-extra"), "snmalloc", true),
        (Some("SYSTEM"), "snmalloc", true),
    ] {
        let mut child = Command::new(std::env::current_exe().unwrap());
        child
            .args(["--exact", "launch_selects_the_backend_for_all_threads"])
            .env(EXPECTED, expected);
        if let Some(value) = setting {
            child.env("RALLOCATOR_DYNAMIC_PERSONALITY", value);
        } else {
            child.env_remove("RALLOCATOR_DYNAMIC_PERSONALITY");
        }
        let output = child.output().unwrap();
        assert!(output.status.success(), "setting {setting:?}: {output:?}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(
            stderr.matches("unknown RALLOCATOR_DYNAMIC_PERSONALITY").count(),
            usize::from(warn),
            "{stderr}"
        );
    }
}

#[expect(
    clippy::unwrap_used,
    reason = "The child must complete the cross-thread transfer and decode its snapshot"
)]
fn exercise(expected: &str) {
    // The global allocator is already in use by the harness. Changing the
    // Windows environment is safe even with harness threads; it is not on Unix.
    #[cfg(windows)]
    // SAFETY: Windows serializes access to its process environment.
    unsafe {
        std::env::set_var(
            "RALLOCATOR_DYNAMIC_PERSONALITY",
            if expected == "system" { "mimalloc" } else { "system" },
        );
    }
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
        let has_source = |id| decoded.sources.iter().any(|entry| entry.id == id);
        assert_eq!(has_source(source::ID), expected == "snmalloc");
        assert_eq!(has_source(source::MIMALLOC_ID), expected == "mimalloc");
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
