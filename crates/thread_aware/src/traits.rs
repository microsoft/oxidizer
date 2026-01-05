// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Traits for thread-aware types.
//!
//! This module provides a separate reexport of the [`ThreadAware`] trait, which is also
//! available at the crate root. The purpose of this module is to allow downstream crates
//! to selectively reexport just the trait without also bringing in the [`ThreadAware`]
//! derive macro (which is conditionally exported at the crate root when the `derive`
//! feature is enabled).
//!
//! # Usage
//!
//! ```rust
//! // Import just the trait from this module (no derive macro)
//! use thread_aware::traits::ThreadAware;
//! # use thread_aware::affinity::{MemoryAffinity, PinnedAffinity};
//! # struct MyType;
//! # impl ThreadAware for MyType {
//! #     fn relocated(self, _: MemoryAffinity, _: PinnedAffinity) -> Self { self }
//! # }
//!
//! // Alternatively, use the root-level export
//! // use thread_aware::ThreadAware;
//! ```
//!
//! Both imports reference the same trait, but importing from this module allows
//! you to avoid coupling to the procedural macro dependency.

#[doc(inline)]
pub use crate::core::ThreadAware;
