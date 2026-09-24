// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Placement for read-mostly allocator globals, not padding for every record.
//! The 64-byte placement targets the measured x86-64 workload, not a universal
//! coherence-line guarantee. Keeping the domain head and aggregate-availability
//! flag away from mutable counters avoids their measured
//! co-location without changing atomic operations or allocating storage.

use std::ops::Deref;

#[derive(Debug)]
#[repr(align(64))]
pub(crate) struct CacheLine<T>(T);

impl<T> CacheLine<T> {
    pub(crate) const fn new(value: T) -> Self {
        Self(value)
    }
}

impl<T> Deref for CacheLine<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use std::mem::offset_of;
    use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};

    use super::CacheLine;

    #[test]
    fn read_mostly_cells_fill_one_line_with_the_atomic_at_its_start() {
        let cell = CacheLine::new(AtomicBool::new(true));
        assert_eq!(
            (
                size_of::<CacheLine<AtomicBool>>(),
                align_of::<CacheLine<AtomicBool>>(),
                offset_of!(CacheLine<AtomicBool>, 0),
                size_of::<CacheLine<AtomicPtr<()>>>(),
                align_of::<CacheLine<AtomicPtr<()>>>(),
                offset_of!(CacheLine<AtomicPtr<()>>, 0),
                cell.load(Ordering::Relaxed),
                std::ptr::from_ref(&cell).addr() % align_of::<CacheLine<AtomicBool>>(),
            ),
            (64, 64, 0, 64, 64, 0, true, 0)
        );
    }
}
