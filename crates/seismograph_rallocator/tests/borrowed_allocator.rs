// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Verify producer serialization cannot call the application's global allocator.

// Allocation counting runs natively and under cargo careful; native.rs retains
// borrowed codec and error-path coverage under Miri without replacing its allocator.
#![cfg(not(miri))]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use seismograph_rallocator::native::{Observation, ObservationSource, Owner, Snapshot};
use seismograph_rallocator::{ErrorKind, MAX_OWNERS, decode, encode, encode_with_owners, encoded_len, encoded_len_with_owners};

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

#[test]
fn truncated_inventory_is_rejected_before_allocating_for_valid_prefix_rows() {
    let metadata = Snapshot {
        owner_count: 2,
        owners: vec![Owner { id: 1, ..Owner::default() }],
        ..Snapshot::default()
    };
    let mut input = vec![0; encoded_len(&metadata).unwrap()];
    encode(&metadata, &mut input).unwrap();
    input[46..50].copy_from_slice(&2_u32.to_le_bytes());
    CALLS.with(|calls| calls.set(Some(0)));
    let decoded = decode(&input);
    let calls = CALLS.with(|calls| calls.replace(None).unwrap());
    assert_eq!(decoded.unwrap_err().kind(), ErrorKind::Truncated);
    assert_eq!(calls, 0);
}

#[test]
fn forged_maximum_inventory_with_invalid_first_owner_does_not_allocate() {
    let metadata = Snapshot::default();
    let header_length = encoded_len(&metadata).unwrap();
    let mut input = vec![0; header_length];
    encode(&metadata, &mut input).unwrap();
    input[36..44].copy_from_slice(&(MAX_OWNERS as u64).to_le_bytes());
    input[46..50].copy_from_slice(&u32::try_from(MAX_OWNERS).unwrap().to_le_bytes());
    input.resize(header_length + MAX_OWNERS * 19, 0);
    CALLS.with(|calls| calls.set(Some(0)));
    let decoded = decode(&input);
    let calls = CALLS.with(|calls| calls.replace(None).unwrap());
    assert_eq!(decoded.unwrap_err().kind(), ErrorKind::DuplicateOwner);
    assert_eq!(calls, 0);
}
