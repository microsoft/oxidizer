// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Pointer-metadata helpers for allocating dynamically sized values.
//!
//! This module re-exports the [`ptr_meta`] vocabulary used by the `dst`
//! feature. The allocation APIs live on [`Arena`](crate::Arena):
//! [`Arena::alloc_dst_arc`](crate::Arena::alloc_dst_arc),
//! [`Arena::alloc_dst_rc`](crate::Arena::alloc_dst_rc), and
//! [`Arena::alloc_dst_box`](crate::Arena::alloc_dst_box), with fallible and
//! pinned variants for each ownership model.
//!
//! Callers provide an exact [`core::alloc::Layout`], matching pointer metadata,
//! and an initializer that writes a valid value. This supports slices and trait
//! objects without storing a wide pointer in each smart-pointer handle.
//! Metadata is stored with the arena allocation and reconstructed when the
//! value is dereferenced or dropped.
//!
//! # Safety and failure behavior
//!
//! DST construction is unsafe because a mismatched layout, metadata value, or
//! initializer can create an invalid value or write outside the reservation.
//! The `try_*` methods report allocation, layout-overflow, and unsupported
//! alignment failures through [`crate::AllocError`]; infallible methods panic
//! for the same allocation failures. Once constructed, the returned smart
//! pointers provide their normal safe ownership and drop behavior.
//!
//! # Example
//!
//! ```
//! # #[cfg(feature = "dst")] {
//! use core::alloc::Layout;
//!
//! use multitude::Arena;
//!
//! let arena = Arena::new();
//! let source = [10_u32, 20, 30];
//! let Ok(layout) = Layout::array::<u32>(source.len()) else {
//!     panic!("slice layout overflow");
//! };
//! // SAFETY: the layout and metadata describe the copied slice exactly.
//! let value = unsafe {
//!     arena.alloc_dst_box::<[u32]>(layout, source.len(), |destination| {
//!         core::ptr::copy_nonoverlapping(
//!             source.as_ptr(),
//!             destination.cast::<u32>(),
//!             source.len(),
//!         );
//!     })
//! };
//! assert_eq!(&*value, &source);
//! # }
//! ```

pub use ptr_meta::{DynMetadata, Pointee, metadata, pointee};
