// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::io::Read;
use std::ptr::NonNull;

use super::*;
use crate::backend::{mimalloc, snmalloc};

#[cfg(unix)]
fn lookup(setting: Option<&str>) -> Setting {
    let value = setting.map(|name| std::ffi::CString::new(name).unwrap());
    // SAFETY: The optional C string remains live throughout parsing.
    unsafe {
        env::read_with(|key| {
            assert_eq!(key, c"RALLOCATOR_DYNAMIC_PERSONALITY");
            value.as_ref().map_or(std::ptr::null(), |s| s.as_ptr())
        })
    }
}

#[cfg(windows)]
fn lookup(setting: Option<&str>) -> Setting {
    let value: Vec<_> = setting.unwrap_or_default().encode_utf16().collect();
    env::read_with(|key, buffer| {
        for (i, unit) in "RALLOCATOR_DYNAMIC_PERSONALITY\0".encode_utf16().enumerate() {
            // SAFETY: The lookup supplies a NUL-terminated key. All preceding units matched and were nonzero.
            assert_eq!(unsafe { key.wrapping_add(i).read() }, unit);
        }
        // GetEnvironmentVariableW reports the required capacity rather than copying a truncated prefix.
        if value.len() >= buffer.len() {
            return value.len() + 1;
        }
        buffer[..value.len()].copy_from_slice(&value);
        value.len()
    })
}

#[test]
fn losing_initializer_uses_the_backend_published_during_its_lookup() {
    let cache = AtomicU8::new(0);
    let loser = initialize(
        &cache,
        Choice::System,
        || {
            let winner = initialize(
                &cache,
                Choice::Snmalloc,
                || Setting::Known(Choice::Mimalloc),
                |_| panic!("valid setting"),
            );
            assert_eq!(winner, Choice::Mimalloc);
            Setting::Unknown
        },
        |_| panic!("only the winner reports an invalid setting"),
    );
    assert_eq!(loser, Choice::Mimalloc);
}

#[test]
fn each_setting_freezes_one_backend_and_warns_only_on_initialization() {
    for (setting, default, expected, warns) in [
        (None, Choice::System, Choice::System, false),
        (None, Choice::Snmalloc, Choice::Snmalloc, false),
        (Some(""), Choice::Mimalloc, Choice::Mimalloc, false),
        (Some("snmalloc"), Choice::Mimalloc, Choice::Snmalloc, false),
        (Some("mimalloc"), Choice::System, Choice::Mimalloc, false),
        (Some("system"), Choice::Snmalloc, Choice::System, false),
        (Some("mimalloc-extra"), Choice::System, Choice::System, true),
        (Some("SYSTEM"), Choice::Snmalloc, Choice::Snmalloc, true),
        (Some("bogus"), Choice::Mimalloc, Choice::Mimalloc, true),
    ] {
        let cache = AtomicU8::new(0);
        let mut warning = None;
        let first = selected(&cache, |cache| {
            initialize(cache, default, || lookup(setting), |fallback| warning = Some(fallback))
        });
        assert_eq!((first, warning), (expected, warns.then_some(expected)));

        let layout = Layout::from_size_align(83, 64).unwrap();
        let address = std::thread::scope(|scope| {
            scope
                .spawn(|| {
                    let owner = selected(&cache, |_| panic!("a second thread must not read the environment again"));
                    assert_eq!(owner, first);
                    // SAFETY: The checked nonzero layout is valid for this backend.
                    let ptr = NonNull::new(unsafe { owner.allocate::<true>(layout) }).unwrap();
                    // SAFETY: The zeroed block is exclusively owned by this thread.
                    let bytes = unsafe { std::slice::from_raw_parts_mut(ptr.as_ptr(), layout.size()) };
                    assert_eq!(bytes, &[0; 83]);
                    bytes.fill(0xb7);
                    owner.publish_state();
                    // A fresh worker has a native owner only for the allocator it actually used.
                    assert_eq!(snmalloc::has_current_owner(), expected == Choice::Snmalloc);
                    assert_eq!(mimalloc::has_current_owner(), expected == Choice::Mimalloc);
                    ptr.as_ptr().expose_provenance()
                })
                .join()
                .unwrap()
        });
        let ptr = std::ptr::with_exposed_provenance_mut(address);
        assert_eq!(address % layout.align(), 0);
        let grown = Layout::from_size_align(9001, layout.align()).unwrap();
        // SAFETY: The worker transferred its live block. This valid size exceeds the supported address space.
        assert!(unsafe { first.reallocate_recorded(ptr, layout, 1 << 62, |_| panic!("failed resize recorded")) }.is_null());
        // SAFETY: Failure leaves the original block and its layout live.
        let retained = unsafe { first.reallocate_recorded(ptr, layout, layout.size(), |_| panic!("unchanged resize recorded")) };
        assert_eq!(retained.addr(), address);
        let mut recorded = None;
        // SAFETY: The unchanged resize returned the live block. The callback only saves its replacement address.
        let new =
            NonNull::new(unsafe { first.reallocate_recorded(retained, layout, grown.size(), |p| recorded = Some(p.addr())) }).unwrap();
        assert_eq!(recorded, Some(new.addr().get()));
        assert_ne!(new.addr().get(), address);
        assert_eq!(new.addr().get() % grown.align(), 0);
        // SAFETY: The move preserved the initialized prefix.
        assert_eq!(unsafe { std::slice::from_raw_parts(new.as_ptr(), layout.size()) }, &[0xb7; 83]);
        // SAFETY: The same backend owns the grown block and receives a valid size.
        let small = NonNull::new(unsafe { first.reallocate(new.as_ptr(), grown, layout.size()) }).unwrap();
        // SAFETY: The unrecorded resize preserves the common prefix as well.
        assert_eq!(unsafe { std::slice::from_raw_parts(small.as_ptr(), layout.size()) }, &[0xb7; 83]);
        // SAFETY: The final block is live and returns to its creating backend exactly once.
        unsafe { first.deallocate(small.as_ptr(), layout) };
    }
}

#[test]
fn invalid_setting_writes_the_fallback_diagnostic_to_the_supplied_output() {
    for (default, name) in [
        (Choice::System, "system"),
        (Choice::Snmalloc, "snmalloc"),
        (Choice::Mimalloc, "mimalloc"),
    ] {
        let (mut reader, writer) = std::io::pipe().unwrap();
        #[cfg(unix)]
        let output = std::os::fd::AsRawFd::as_raw_fd(&writer);
        #[cfg(windows)]
        let output = std::os::windows::io::AsRawHandle::as_raw_handle(&writer);
        let cache = AtomicU8::new(0);
        assert_eq!(
            initialize(
                &cache,
                default,
                || lookup(Some("invalid")),
                env::stderr_warning(|_, fallback| env::write_warning(output, fallback))
            ),
            default
        );
        drop(writer);
        let mut message = String::new();
        reader.read_to_string(&mut message).unwrap();
        assert_eq!(
            message,
            format!("rallocator: unknown RALLOCATOR_DYNAMIC_PERSONALITY. Use snmalloc, mimalloc, or system. Using {name}.\n")
        );
    }
}
