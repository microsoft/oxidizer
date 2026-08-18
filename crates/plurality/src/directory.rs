// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Growth for the vectors a pool mutates while an allocator call may be
//! outstanding.
//!
//! A pool keeps its chunk directory, and a multi pool its layout directory, in
//! an [`UnsafeCell<Vec<_>>`]. Growing such a vector through `Vec` itself holds
//! `&mut` across a call into the global allocator, and an allocator that
//! allocates from the pool it serves would take a second borrow of the same
//! vector. Growth here never holds a borrow across an allocator call, and
//! hands the displaced buffer back so the caller can free it once its own
//! publication is complete. See `docs/implementation/reentrancy.md`.

#![expect(
    clippy::multiple_unsafe_ops_per_block,
    reason = "each block reads or moves through one `UnsafeCell` several times under a single caller-guaranteed precondition that no other borrow is live; one block per operation would repeat that precondition per statement, and the buffer swap has no intermediate state a separate block could describe"
)]

use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::{mem, ptr};

use crate::error::AllocError;

/// A buffer that [`reserve_one`] displaced, owned by the caller until dropped.
///
/// Freeing calls the global allocator, which is a point where control leaves
/// the pool. Handing the buffer back lets the caller place that call after the
/// state update the reservation was made for, so no allocator call separates
/// the reservation from the push it guarantees. Ref:
/// docs/implementation/reentrancy.md, "Reserving without a live borrow".
///
/// Carries no elements: growth moves them to the new buffer and leaves the old
/// one empty, so dropping this releases memory without touching values.
pub(crate) struct Displaced<T>(#[expect(dead_code, reason = "held solely so the caller controls when the buffer is freed")] Vec<T>);

/// `true` if one more element fits in `cell` without reallocating.
///
/// # Safety
/// No borrow of `cell` may be live, and the caller must hold the pool's
/// single-threaded allocation path.
pub(crate) unsafe fn has_room<T>(cell: &UnsafeCell<Vec<T>>) -> bool {
    // SAFETY: the caller guarantees no other borrow is live, and this one ends
    // with the statement.
    unsafe { (*cell.get()).len() < (*cell.get()).capacity() }
}

/// Reserves room in `cell` for one more element.
///
/// The returned buffer is freed on drop. Hold it until the element has been
/// pushed.
///
/// # Safety
/// No borrow of `cell` may be live, and the caller must hold the pool's
/// single-threaded allocation path.
pub(crate) unsafe fn reserve_one<T>(cell: &UnsafeCell<Vec<T>>) -> Result<Displaced<T>, AllocError> {
    let mut previous = None;
    loop {
        // SAFETY: the caller guarantees no other borrow is live, and this one
        // ends with the statement.
        let capacity = unsafe { (*cell.get()).capacity() };
        // SAFETY: as above.
        if unsafe { (*cell.get()).len() } < capacity {
            return Ok(Displaced(Vec::new()));
        }

        // A retry means a reentrant push outgrew the buffer prepared for it,
        // which it can only do by growing the vector past that buffer's
        // capacity. Capacity is therefore strictly increasing across
        // iterations and is itself bounded by the pool's chunk or layout cap,
        // so the loop terminates.
        debug_assert!(previous.is_none_or(|seen| capacity > seen), "reservation retried without growth");
        previous = Some(capacity);

        // Doubling matches `Vec`'s own growth, keeping repeated reservation
        // amortized constant. The floor gives the first growth a useful buffer.
        let target = if capacity == 0 { 4 } else { capacity.saturating_mul(2) };
        let mut fresh = Vec::new();
        fresh.try_reserve_exact(target).map_err(|_err| AllocError::ALLOCATOR_FAILED)?;

        // The allocation above is the last point control leaves this function,
        // so re-read the length it may have changed. Everything below is pure
        // memory movement.
        //
        // A reentrant push may have grown `cell` to a buffer that already has
        // room, making the copy below a waste of a fresh allocation. Returning
        // early on that observation was considered and rejected: it trades a
        // branch on every reservation for a saving on an interleaving nothing
        // in the crate can predict the frequency of, and the copy is correct
        // either way.
        // SAFETY: as above.
        let live = unsafe { (*cell.get()).len() };
        if live >= target {
            continue;
        }

        // SAFETY: as above; `live` is the current length and `fresh` has
        // `target > live` capacity, so the copy stays in bounds of both. The
        // elements are moved, not duplicated: the source is emptied before it
        // is handed back, so only the buffer is freed.
        let displaced = unsafe {
            let current = &mut *cell.get();
            ptr::copy_nonoverlapping(current.as_ptr(), fresh.as_mut_ptr(), live);
            fresh.set_len(live);
            current.set_len(0);
            mem::replace(current, fresh)
        };
        return Ok(Displaced(displaced));
    }
}
