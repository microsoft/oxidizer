// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Verify producer serialization cannot call the application's global allocator.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use seismograph_rallocator::native::{Observation, ObservationSource, Owner, Snapshot};
use seismograph_rallocator::{ErrorKind, encode, encode_with_owners, encoded_len, encoded_len_with_owners};

thread_local! {
    static CALLS: Cell<Option<usize>> = const { Cell::new(None) };
}

struct TrackingSystem;

#[global_allocator]
static ALLOCATOR: TrackingSystem = TrackingSystem;

fn observe() {
    // TLS may already be unavailable during teardown; only the test scope counts.
    let _ = CALLS.try_with(|calls| {
        if let Some(count) = calls.get() {
            calls.set(Some(count.saturating_add(1)));
        }
    });
}

// SAFETY: All operations forward pointer ownership and layouts unchanged to System.
unsafe impl GlobalAlloc for TrackingSystem {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        observe();
        // SAFETY: The GlobalAlloc caller supplies a valid allocation layout.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        observe();
        // SAFETY: The caller returns a System allocation with its original layout.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        observe();
        // SAFETY: The caller supplies the original allocation and valid new size.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[test]
fn borrowed_length_encoding_and_error_paths_never_use_global_allocator() {
    let metadata = Snapshot {
        owner_count: 1025,
        owners_complete: true,
        ..Snapshot::default()
    };
    let mut owners = (1..=1025).map(|id| Owner { id, ..Owner::default() }).collect::<Vec<_>>();
    owners[0].source = ObservationSource::Published;
    owners[0].observation = Some(Observation::EMPTY);
    let length = encoded_len_with_owners(&metadata, &owners).unwrap();
    let mut output = vec![0; length];
    let owned = Snapshot {
        owners: owners.clone(),
        ..metadata
    };
    CALLS.with(|calls| calls.set(Some(0)));
    let measured_length = encoded_len_with_owners(&metadata, &owners);
    let encoded = encode_with_owners(&metadata, &owners, &mut output);
    let owned_length = encoded_len(&owned);
    let owned_encoded = encode(&owned, &mut output);
    let wrong_length = encode_with_owners(&metadata, &owners, &mut output[..length - 1]);
    owners[1024].id = owners[0].id;
    let duplicate = encoded_len_with_owners(&metadata, &owners);
    let calls = CALLS.with(|calls| calls.replace(None).unwrap());
    assert_eq!(calls, 0);
    assert_eq!(measured_length.unwrap(), length);
    assert_eq!(encoded, Ok(()));
    assert_eq!(owned_length.unwrap(), length);
    assert_eq!(owned_encoded.unwrap(), length);
    assert_eq!(wrong_length.unwrap_err().kind(), ErrorKind::LengthMismatch);
    assert_eq!(duplicate.unwrap_err().kind(), ErrorKind::DuplicateOwner);
}
