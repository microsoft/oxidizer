// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Helpers for building dynamically sized arena values.
//!
//! Start with [`Arena::alloc_dst_arc`](crate::Arena::alloc_dst_arc) or
//! [`Arena::alloc_dst_box`](crate::Arena::alloc_dst_box).

pub use ptr_meta::{DynMetadata, Pointee, metadata, pointee};
