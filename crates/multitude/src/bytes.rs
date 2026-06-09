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
        Self::from_owner(arc)
    }
}

impl<A> From<Arc<str, A>> for Bytes
where
    A: Allocator + Clone + Send + Sync + 'static,
{
    /// Convert an arena-allocated [`Arc<str, A>`](crate::Arc) into a
    /// [`Bytes`] without copying.
    ///
    /// The conversion routes through `Arc<[u8], A>` via the
    /// `Arc<str> → Arc<[u8]>` retag (O(1), no copy) and wraps the
    /// result in `Bytes`.
    #[inline]
    fn from(s: Arc<str, A>) -> Self {
        let arc_bytes: Arc<[u8], A> = s.into();
        Self::from_owner(arc_bytes)
    }
}
