// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Arena-backed growable vectors and the [`vec!`](macro@vec) macro.
//!
//! [`Vec`] provides the familiar growable-array operations while obtaining its
//! storage from an [`Arena`]. Create one with [`Arena::alloc_vec`], collect an
//! iterator with [`CollectIn`], or use [`vec!`](macro@vec). The vector borrows
//! its arena, so it cannot outlive the arena or overlap a mutable arena reset.
//!
//! Growth first attempts to extend the most recent bump allocation in place.
//! Otherwise it reserves a larger arena region and moves live elements there;
//! the abandoned region is reclaimed with its chunk or when the arena is
//! reset. Consuming freeze operations can turn eligible buffers into
//! [`crate::Box`], [`crate::Rc`], or [`crate::Arc`] slices without copying.
//! Element destructors still run when the vector or its consuming iterator is
//! dropped.
//!
//! Capacity growth can panic when allocation fails. Fallible `try_*` methods
//! return [`crate::AllocError`] and preserve the documented state of their
//! inputs. Indexing, insertion, draining, and splicing use byte-for-byte
//! `std::vec::Vec`-style bounds behavior.
//!
//! # Example
//!
//! ```
//! use multitude::Arena;
//! use multitude::vec::{CollectIn as _, Vec};
//!
//! let arena = Arena::new();
//! let mut values = multitude::vec::vec![in &arena; 1, 2, 3];
//! values.push(4);
//! assert_eq!(values.as_slice(), &[1, 2, 3, 4]);
//!
//! let squares: Vec<i32> = (1..=3).map(|value| value * value).collect_in(&arena);
//! assert_eq!(squares.as_slice(), &[1, 4, 9]);
//! ```

use core::marker::PhantomData;
use core::mem;

use allocator_api2::alloc::{Allocator, Global};

use crate::internal::arena_buf::ArenaBuf;
use crate::internal::constants::buffer_freezable;
use crate::{AllocError, Arena};

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
    buf: ArenaBuf<'a, T>,
    arena: &'a Arena<A>,
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
    const fn from_buf(buf: ArenaBuf<'a, T>, arena: &'a Arena<A>) -> Self {
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
    fn try_grow_to(&mut self, new_cap: usize) -> Result<(), AllocError> {
        if const { mem::size_of::<T>() == 0 } {
            return Ok(());
        }
        debug_assert!(new_cap > self.buf.cap(), "try_grow_to: callers must ensure new_cap > current cap");
        // Reject capacities whose raw payload overflows `usize` up front so
        // both buffer kinds surface a recoverable `AllocError`, rather than the
        // freezable hint saturating into the oversized path (where the in-chunk
        // reservation would later overflow and panic on the `expect` below).
        let payload_bytes = mem::size_of::<T>().checked_mul(new_cap).ok_or(AllocError::CAPACITY_OVERFLOW)?;
        let refill_hint = if const { buffer_freezable::<T>() } {
            // Freezable buffers carry the `Arc<[T]>` freeze prefix (the
            // superset of `Rc`'s, so a freeze into either reuses it); the
            // refill hint must budget for it so the chunk fits prefix +
            // payload + alignment slack.
            crate::arena::alloc_prefixed::worst_case_strong_slice_payload::<crate::internal::thin_dst::AtomicStrong, T>(new_cap)
        } else {
            payload_bytes
                .checked_add(mem::align_of::<T>())
                .ok_or(AllocError::CAPACITY_OVERFLOW)?
        };
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
            let reserved = if const { buffer_freezable::<T>() } {
                self.arena.try_reserve_freezable_slice::<T>(new_cap)
            } else {
                self.arena.try_reserve_local_slice::<T>(new_cap)
            };
            if let Some(u) = reserved {
                break u;
            }
            if self.arena.is_oversized(refill_hint) {
                let (new_ptr, new_cap_actual) = self.arena.alloc_oversized_local_with(refill_hint, |mutator| {
                    let ticket = if const { buffer_freezable::<T>() } {
                        mutator.try_alloc_freezable_slice::<T>(new_cap)
                    } else {
                        mutator.try_alloc_uninit_slice::<T>(new_cap)
                    };
                    ticket
                        .expect("dedicated oversized chunk sized to fit growable buffer")
                        .into_raw_buffer()
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
            self.arena.refill(refill_hint)?;
        };
        #[cfg(feature = "stats")]
        if old_cap > 0 {
            self.arena.record_relocation();
        }
        self.buf.replace_buffer(uninit);
        Ok(())
    }
}

impl<T, A: Allocator + Clone> Drop for Vec<'_, T, A> {
    #[inline]
    fn drop(&mut self) {
        // Drop elements before reclaiming storage so reentrant allocations
        // cannot overwrite elements still being dropped. Reclaim succeeds only
        // while this buffer remains at the current chunk's cursor.
        self.buf.truncate(0);
        let _ = self.reclaim_capacity_tail(0);
    }
}
