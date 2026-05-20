// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Safe zero-initialized arena allocations for `zerocopy::FromZeros` types.
//!
//! # Usage
//!
//! Access is through the [`ZerocopyView`] obtained via [`Arena::zerocopy()`](crate::Arena::zerocopy):
//!
//! ```
//! # #[cfg(feature = "zerocopy")] {
//! use multitude::Arena;
//! use zerocopy::FromZeros;
//!
//! #[derive(FromZeros)]
//! struct Header {
//!     version: u32,
//!     flags: u64,
//! }
//!
//! let arena = Arena::new();
//! let header = arena.zerocopy().alloc_rc::<Header>();
//! assert_eq!(header.version, 0);
//! assert_eq!(header.flags, 0);
//! # }
//! ```

use crate::zero_init_macros::zero_init_view;

zero_init_view!(ZerocopyView, zerocopy::FromZeros, "zerocopy: arena allocation failed");
