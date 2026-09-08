// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#[cfg(feature = "seismograph")]
use core::sync::atomic::{AtomicU64, Ordering};

#[cfg(feature = "seismograph")]
use seismograph_io::BufferId;

#[cfg(feature = "seismograph")]
#[derive(Debug, Default)]
pub(crate) struct BufferIdentity(AtomicU64);

#[cfg(not(feature = "seismograph"))]
#[derive(Debug, Default)]
pub(crate) struct BufferIdentity;

#[cfg(feature = "seismograph")]
pub(crate) type TransferredIdentity = Option<BufferId>;

#[cfg(not(feature = "seismograph"))]
#[derive(Clone, Copy, Debug)]
pub(crate) struct TransferredIdentity;

impl BufferIdentity {
    pub(crate) const fn new() -> Self {
        #[cfg(feature = "seismograph")]
        {
            Self(AtomicU64::new(0))
        }
        #[cfg(not(feature = "seismograph"))]
        {
            Self
        }
    }

    pub(crate) const fn empty_transfer() -> TransferredIdentity {
        #[cfg(feature = "seismograph")]
        {
            None
        }
        #[cfg(not(feature = "seismograph"))]
        {
            TransferredIdentity
        }
    }

    #[cfg(feature = "seismograph")]
    pub(crate) fn get_or_allocate(&self) -> BufferId {
        let current = self.0.load(Ordering::Relaxed);
        if let Some(id) = BufferId::from_raw(current) {
            return id;
        }

        let allocated = BufferId::allocate();
        let selected = self
            .0
            .compare_exchange(0, allocated.get(), Ordering::Relaxed, Ordering::Relaxed)
            .map_or_else(identity_after_allocation_race, |_| allocated.get());
        BufferId::from_raw(selected).expect("buffer identity can only transition from zero to a valid ID")
    }

    pub(crate) fn take(&self) -> TransferredIdentity {
        #[cfg(feature = "seismograph")]
        {
            BufferId::from_raw(self.0.swap(0, Ordering::Relaxed))
        }
        #[cfg(not(feature = "seismograph"))]
        {
            let _ = self;
            TransferredIdentity
        }
    }

    pub(crate) fn replace(&self, id: TransferredIdentity) {
        #[cfg(feature = "seismograph")]
        self.0.store(id.map_or(0, BufferId::get), Ordering::Relaxed);
        #[cfg(not(feature = "seismograph"))]
        {
            let _ = (self, id);
        }
    }

    pub(crate) fn clear(&self) {
        #[cfg(feature = "seismograph")]
        self.0.store(0, Ordering::Relaxed);
        #[cfg(not(feature = "seismograph"))]
        let _ = self;
    }
}

#[cfg(feature = "seismograph")]
#[cfg_attr(coverage_nightly, coverage(off))] // Requires winning the narrow compare-exchange race after both callers observe zero.
const fn identity_after_allocation_race(existing: u64) -> u64 {
    existing
}

#[cfg(all(test, feature = "seismograph"))]
mod tests {
    use super::*;

    #[test]
    fn identity_transfer_clear_and_clone_have_exact_semantics() {
        assert_eq!(identity_after_allocation_race(42), 42);
        let identity = BufferIdentity::new();
        let original = identity.get_or_allocate();
        assert_eq!(identity.get_or_allocate(), original);

        let transferred = identity.take();
        assert_eq!(transferred, Some(original));
        let replacement = identity.get_or_allocate();
        assert_ne!(replacement, original);

        identity.replace(transferred);
        assert_eq!(identity.get_or_allocate(), original);
        identity.clear();
        assert_ne!(identity.get_or_allocate(), original);
    }
}
