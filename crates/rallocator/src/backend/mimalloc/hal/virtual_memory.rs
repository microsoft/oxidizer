// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};

use super::Reservation;

/// Native VM operations with per-instance, failure-only test injection.
/// Successful operations always use real mappings and preserve OS ownership.
pub(in super::super) struct VirtualMemory {
    #[cfg(test)]
    reserve_failures: usize,
    #[cfg(test)]
    commit_failures: usize,
    #[cfg(test)]
    reserve_calls: AtomicUsize,
    #[cfg(test)]
    commit_calls: AtomicUsize,
    #[cfg(test)]
    reservations: [AtomicUsize; 4],
}

impl VirtualMemory {
    pub(in super::super) const fn native() -> Self {
        Self {
            #[cfg(test)]
            reserve_failures: 0,
            #[cfg(test)]
            commit_failures: 0,
            #[cfg(test)]
            reserve_calls: AtomicUsize::new(0),
            #[cfg(test)]
            commit_calls: AtomicUsize::new(0),
            #[cfg(test)]
            reservations: [const { AtomicUsize::new(0) }; 4],
        }
    }

    #[cfg(test)]
    pub(in super::super) fn with_failures(reserve_failures: usize, commit_failures: usize) -> Self {
        Self {
            reserve_failures,
            commit_failures,
            ..Self::native()
        }
    }

    #[cfg_attr(
        not(test),
        expect(
            clippy::unused_self,
            reason = "The production VM is stateless; tests attach per-instance failure plans"
        )
    )]
    pub(in super::super) fn reserve(&self, size: usize, alignment: usize, committed: bool) -> Option<Reservation> {
        #[cfg(test)]
        {
            let call = self.reserve_calls.fetch_add(1, Ordering::Relaxed);
            if let Some(record) = self.reservations.get(call) {
                record.store(size, Ordering::Relaxed);
            }
            if fails(self.reserve_failures, call) {
                return None;
            }
        }
        if committed {
            super::reserve_committed(size, alignment)
        } else {
            super::reserve(size, alignment)
        }
    }

    /// # Safety
    /// The caller coordinates this range and its straddling OS pages with all
    /// accesses, as required by `Reservation::commit`.
    #[cfg_attr(
        not(test),
        expect(
            clippy::unused_self,
            reason = "The production VM is stateless; tests attach per-instance failure plans"
        )
    )]
    pub(in super::super) unsafe fn commit(&self, reservation: &Reservation, offset: usize, size: usize) -> bool {
        #[cfg(test)]
        if fails(self.commit_failures, self.commit_calls.fetch_add(1, Ordering::Relaxed)) {
            return false;
        }
        // SAFETY: The caller provides the reservation's commitment contract.
        unsafe { reservation.commit(offset, size) }
    }

    #[cfg(test)]
    pub(in super::super) fn calls(&self) -> (usize, usize, [usize; 4]) {
        (
            self.reserve_calls.load(Ordering::Relaxed),
            self.commit_calls.load(Ordering::Relaxed),
            std::array::from_fn(|index| self.reservations[index].load(Ordering::Relaxed)),
        )
    }
}

#[cfg(test)]
fn fails(mask: usize, call: usize) -> bool {
    call < usize::BITS as usize && mask & (1 << call) != 0
}

#[cfg(test)]
mod tests {
    use super::VirtualMemory;

    #[test]
    fn reservation_logging_stays_bounded_while_failures_continue() {
        let vm = VirtualMemory::with_failures(usize::MAX, 0);
        let failed = std::array::from_fn::<_, 5, _>(|_| vm.reserve(65536, 65536, false).is_none());

        assert_eq!((failed, vm.calls()), ([true; 5], (5, 0, [65536; 4])));
    }
}
