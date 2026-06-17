// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(
    clippy::items_after_statements,
    clippy::single_match_else,
    clippy::panic,
    clippy::manual_assert,
    clippy::undocumented_unsafe_blocks,
    clippy::match_wild_err_arm,
    clippy::missing_panics_doc,
    clippy::manual_saturating_arithmetic,
    clippy::struct_field_names,
    reason = "reachable panics are programmer error; keep this close to `allocator_api2::vec::Vec`; `drain_*` names mirror `std::vec::Drain`"
)]

//! Arena-backed growable vectors and the `vec!` macro.

use core::marker::PhantomData;
use core::mem;

use allocator_api2::alloc::{AllocError, Allocator, Global};

use crate::Arena;
use crate::internal::arena_buf::ArenaBuf;

mod basic;
mod collect_in;
mod drain;
mod freeze;
mod from_in;
mod from_iterator_in;
mod into_iter;
mod mutate;
mod splice;
mod traits;
mod vec_macro;

pub use collect_in::CollectIn;
pub use drain::Drain;
pub use from_iterator_in::FromIteratorIn;
pub use into_iter::IntoIter;
pub use splice::Splice;

#[doc(inline)]
pub use crate::__multitude_vec as vec;

/// A growable, mutable vector that lives in an [`Arena`].
///
/// `push`, `pop`, `extend`, `iter`, and other standard vector methods
/// behave the same as on `std::vec::Vec`.
///
/// # Example
///
/// ```
/// use multitude::Arena;
///
/// let arena = Arena::new();
/// let mut v = arena.alloc_vec::<i32>();
/// v.push(1);
/// v.push(2);
/// v.push(3);
/// let frozen = v.into_boxed_slice();
/// assert_eq!(&*frozen, &[1, 2, 3]);
/// ```
pub struct Vec<'a, T, A: Allocator + Clone = Global> {
    pub(super) buf: ArenaBuf<'a, T>,
    pub(super) arena: &'a Arena<A>,
    /// Marker for covariance in `T` and to enforce `!Send`/`!Sync`.
    _phantom: PhantomData<*const T>,
}

// `Vec` is `!Send`/`!Sync` because it holds `&Arena<A>` and mutating or drop
// paths call back into that arena. The reference alone blocks the auto traits.

// `Vec`'s owning iterator is defined in the `into_iter` module and
// re-exported above.

impl<'a, T, A: Allocator + Clone> Vec<'a, T, A> {
    /// Construct a `Vec` wrapping the given (empty) buffer.
    #[inline]
    pub(super) const fn from_buf(buf: ArenaBuf<'a, T>, arena: &'a Arena<A>) -> Self {
        Self {
            buf,
            arena,
            _phantom: PhantomData,
        }
    }

    /// Returns the arena this `Vec` borrows from.
    #[inline]
    pub(crate) const fn arena(&self) -> &'a Arena<A> {
        self.arena
    }

    /// Grows the backing buffer to hold at least `new_cap` elements by
    /// reserving a fresh slice in the arena and handing it to the
    /// underlying [`ArenaBuf`], which migrates the live elements and
    /// abandons the old storage (reclaimed when the arena is torn down).
    ///
    /// No-op for ZSTs (their `cap` is `usize::MAX` by construction) and
    /// for requests below the current capacity.
    #[cold]
    #[inline(never)]
    #[cfg_attr(test, mutants::skip)] // `>` vs `>=` observationally equivalent at old_cap == 0
    pub(super) fn try_grow_to(&mut self, new_cap: usize) -> Result<(), AllocError> {
        if const { mem::size_of::<T>() == 0 } {
            return Ok(());
        }
        debug_assert!(new_cap > self.buf.cap(), "try_grow_to: callers must ensure new_cap > current cap");
        let refill_hint = mem::size_of::<T>()
            .checked_mul(new_cap)
            .and_then(|b| b.checked_add(mem::align_of::<T>()))
            .ok_or(AllocError)?;
        let old_cap = self.buf.cap();
        // Fast path: if our storage sits at the chunk's bump cursor and the
        // chunk has room, extend it in place with no copy and no relocation.
        if old_cap > 0 {
            let elem = mem::size_of::<T>();
            let base = self.buf.as_ptr() as usize;
            if let (Some(old_bytes), Some(new_bytes)) = (old_cap.checked_mul(elem), new_cap.checked_mul(elem))
                && self.arena.try_grow_local_in_place(base, old_bytes, new_bytes)
            {
                // SAFETY: the bump cursor was advanced to cover `new_cap`
                // contiguous elements at the same base pointer, so the
                // capacity can be raised without moving any element.
                unsafe { self.buf.set_cap(new_cap) };
                return Ok(());
            }
        }
        let uninit = loop {
            if let Some(u) = self.arena.try_reserve_local_slice::<T>(new_cap) {
                break u;
            }
            if self.arena.is_oversized(refill_hint) {
                let (new_ptr, new_cap_actual) = self.arena.alloc_oversized_local_with(refill_hint, |mutator| {
                    let ticket = mutator
                        .try_alloc_uninit_slice::<T>(new_cap)
                        .expect("dedicated oversized chunk sized to fit growable buffer");
                    ticket.into_raw_buffer()
                })?;
                #[cfg(feature = "stats")]
                if old_cap > 0 {
                    self.arena.record_relocation();
                }
                // SAFETY: chunk hosting `new_ptr` is retained in
                // `retired_local` for the lifetime of `&Arena` (and
                // hence for `'a`); the reservation is fresh and
                // non-overlapping with the old buffer.
                unsafe { self.buf.replace_buffer_raw(new_ptr, new_cap_actual) };
                // The previous oversized chunk is left in `retired_local`
                // until arena reset/drop. It is NOT released here: a
                // zero-copy `split_off` can leave a sibling buffer
                // pointing into the same chunk, and freeing it on one
                // half's growth would dangle the other (use-after-free).
                return Ok(());
            }
            self.arena.refill_local(refill_hint)?;
        };
        #[cfg(feature = "stats")]
        if old_cap > 0 {
            self.arena.record_relocation();
        }
        self.buf.replace_buffer(uninit);
        Ok(())
    }
}

// `Vec`'s `Drop` runs the live elements' destructors, then hands the
// now-dead storage back to the chunk's bump cursor (a LIFO reclaim) when
// this buffer is still the chunk's last allocation.
impl<T, A: Allocator + Clone> Drop for Vec<'_, T, A> {
    #[inline]
    fn drop(&mut self) {
        // Drop the live elements *first*, while the storage is still
        // reserved: any reentrant arena allocation performed by an
        // element's own `Drop` then lands beyond this buffer rather than
        // over the elements being dropped. Only afterwards do we reclaim
        // the (now-dead) storage, returning it to the bump cursor so
        // later allocations can reuse it instead of waiting for arena
        // teardown. The reclaim is a no-op when the buffer has been
        // overtaken by a later allocation, sits in a retired or oversized
        // chunk, or is a ZST (the cursor-equality guard in
        // `try_reclaim_tail` fails). The `buf` field's own `Drop` runs
        // after this body and re-truncates an already-empty buffer.
        self.buf.truncate(0);
        let _ = self.reclaim_capacity_tail(0);
    }
}
