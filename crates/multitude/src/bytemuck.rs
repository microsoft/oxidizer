// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Safe zero-initialized arena allocations for `bytemuck::Zeroable` types.
//!
//! # Usage
//!
//! Access is through the [`BytemuckView`] obtained via [`Arena::bytemuck()`](crate::Arena::bytemuck):
//!
//! ```
//! # #[cfg(feature = "bytemuck")] {
//! use bytemuck::Zeroable;
//! use multitude::Arena;
//!
//! #[derive(Clone, Copy, Zeroable)]
//! #[repr(C)]
//! struct Pixel {
//!     r: u8,
//!     g: u8,
//!     b: u8,
//!     a: u8,
//! }
//!
//! let arena = Arena::new();
//! let pixel = arena.bytemuck().alloc_rc::<Pixel>();
//! assert_eq!(pixel.r, 0);
//! assert_eq!(pixel.a, 0);
//! # }
//! ```

use crate::zero_init_macros::zero_init_view;

zero_init_view!(BytemuckView, bytemuck::Zeroable, "bytemuck: arena allocation failed");
