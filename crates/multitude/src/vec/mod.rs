// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(
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
//!
//! [`Vec`] is a transient builder that can be frozen into compact arena
//! handles such as [`Vec::into_arena_rc`].
//!
//! For the string equivalents, see [`crate::strings`].

use core::ptr::NonNull;

use allocator_api2::alloc::{Allocator, Global};
use allocator_api2::vec::IntoIter as ApiIntoIter;

use crate::Arena;

mod basic;
mod buffer;
mod collect_in;
mod drain;
mod freeze;
mod from_iterator_in;
mod mutate;
mod traits;
mod vec_macro;

pub use collect_in::CollectIn;
pub use drain::Drain;
pub use from_iterator_in::FromIteratorIn;

#[doc(inline)]
pub use crate::__multitude_vec as vec;

/// A growable, mutable vector that lives in an [`Arena`].
///
/// `Vec` is a **transient builder**: 32 bytes on 64-bit (data pointer +
/// length + capacity + arena reference). Its purpose is to be filled and
/// then frozen via [`Self::into_arena_rc`] into a 16-byte
/// [`Rc<[T], A>`](crate::Rc) — immutable, cloneable, refcounted. For
/// `T: !Drop`, the freeze is **O(1)**.
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
/// let frozen = v.into_arena_rc(); // 32 bytes → 16-byte Rc<[i32]>
/// assert_eq!(&*frozen, &[1, 2, 3]);
/// ```
pub struct Vec<'a, T, A: Allocator + Clone = Global> {
    arena: &'a Arena<A>,
    data: NonNull<T>,
    len: usize,
    cap: usize,
}

// `Vec` is `!Send`/`!Sync` because it holds `&Arena<A>` and mutating or drop
// paths call back into that arena. The reference alone blocks the auto traits.

/// Owning iterator returned by [`Vec::into_iter`].
pub type IntoIter<'a, T, A> = ApiIntoIter<T, &'a Arena<A>>;

impl<T, A: Allocator + Clone> Drop for Vec<'_, T, A> {
    fn drop(&mut self) {
        // `deallocate_buffer` must still run if an element panics in `clear()`;
        // use a guard so unwind does not leak the chunk ref.
        struct DeallocateGuard<'g, 'a, T, A: Allocator + Clone> {
            v: &'g mut Vec<'a, T, A>,
        }
        impl<T, A: Allocator + Clone> Drop for DeallocateGuard<'_, '_, T, A> {
            fn drop(&mut self) {
                Vec::deallocate_buffer(self.v.arena, self.v.data, self.v.cap);
            }
        }
        let g = DeallocateGuard { v: self };
        g.v.clear();
        // `g.drop()` runs on success or unwind.
    }
}
