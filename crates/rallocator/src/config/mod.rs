// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Compile-time allocator configurations.

mod tunables;

pub(crate) use tunables::{MAX_SIZE_CLASSES, SizeClassTables, valid_size_classes};
pub use tunables::{SizeClassLayout, StandardSizeClasses, Tunables};

/// Compile-time allocator configuration.
pub trait Config {
    type Tunables: Tunables;
}

/// Standard allocator configuration.
pub struct Standard;

impl Config for Standard {
    type Tunables = Self;
}
