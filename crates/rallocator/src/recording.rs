// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The allocation event format shared by the backends.
//!
//! [`crate::Rallocator`] rejects failed operations before calling these helpers.

use seismograph::recorder::alloc::{Allocation, AllocationId, EventThreadId, HeapId, HeapKind, Reallocation};
use seismograph::recorder::event::{Address, EventClass, Record};

pub(super) fn emit(address: *mut u8, size: usize, alignment: usize, freed: bool) {
    seismograph::record(EventClass::Allocation, || {
        let allocation = Allocation {
            // This is an address correlation key, not a unique lifetime ID.
            allocation_id: AllocationId::new(address.addr() as u64),
            event_thread_id: EventThreadId::new(0),
            heap_id: HeapId::new(0),
            heap_kind: HeapKind::General,
            freed_after_heap_release: false,
            address: Address::from_ptr(address),
            size: size as u64,
            alignment: alignment as u64,
        };
        Some(if freed {
            Record::deallocation(allocation)
        } else {
            Record::allocation(allocation)
        })
    });
}

pub(super) fn resize(old: *mut u8, new: *mut u8, old_size: usize, new_size: usize, alignment: usize) {
    seismograph::record(EventClass::Allocation, || {
        Some(Record::reallocation(Reallocation {
            allocation_id: AllocationId::new(old.addr() as u64),
            heap_id: HeapId::new(0),
            heap_kind: HeapKind::General,
            old_address: Address::from_ptr(old),
            new_address: Address::from_ptr(new),
            old_size: old_size as u64,
            new_size: new_size as u64,
            alignment: alignment as u64,
        }))
    });
}
