// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Zero-copy conversions from arena-backed byte storage into `bytes::Bytes`.
//!
//! # Usage
//!
//! ```
//! # #[cfg(feature = "bytes")] {
//! use bytes::Bytes;
//! use multitude::Arena;
//!
//! let arena = Arena::new();
//! let arc_bytes = arena.alloc_slice_copy_arc(b"hello world" as &[u8]);
//! let b: Bytes = arc_bytes.into();
//! assert_eq!(&b[..], b"hello world");
//!
//! let arc_str = arena.alloc_str_arc("hello");
//! let b: Bytes = arc_str.into();
//! assert_eq!(&b[..], b"hello");
//! # }
//! ```

use allocator_api2::alloc::Allocator;
use bytes::Bytes;

use crate::Arc;
use crate::strings::ArcStr;

impl<A> From<Arc<[u8], A>> for Bytes
where
    A: Allocator + Clone + Send + Sync + 'static,
{
    /// Convert an arena-allocated [`Arc<[u8], A>`](crate::Arc) into a
    /// [`Bytes`] without copying.
    ///
    /// The arena chunk's refcount keeps the memory alive as long as the
    /// `Bytes` (or any sub-slice of it) exists.
    #[inline]
    fn from(arc: Arc<[u8], A>) -> Self {
        // `Bytes::from_owner` consumes `arc` and keeps it alive for the
        // lifetime of the resulting `Bytes`. Cloning the `Bytes` (or
        // any of its slices) does not clone `arc`; the `bytes` crate
        // tracks the owner with its own reference count.
        Self::from_owner(arc)
    }
}

impl<A> From<ArcStr<A>> for Bytes
where
    A: Allocator + Clone + Send + Sync + 'static,
{
    /// Convert an arena-allocated [`ArcStr<A>`](crate::strings::ArcStr) into a
    /// [`Bytes`] without copying.
    ///
    /// The conversion first reinterprets the string as `Arc<[u8], A>` (O(1)),
    /// then wraps it in `Bytes`.
    #[inline]
    fn from(s: ArcStr<A>) -> Self {
        let arc: Arc<[u8], A> = s.into();
        Self::from(arc)
    }
}
