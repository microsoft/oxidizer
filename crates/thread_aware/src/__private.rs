// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Private module for internal reexports.
//!
//! This module provides a separate reexport of the [`ThreadAware`] trait, which is also
//! available at the crate root. The purpose of this module is to allow downstream crates
//! to selectively reexport just the trait without also bringing in the [`ThreadAware`]
//! derive macro (which is conditionally exported at the crate root when the `derive`
//! feature is enabled).
//!
//! **Note**: This module is not part of the public API and may change without notice.

#[doc(inline)]
pub use crate::core::ThreadAware;
