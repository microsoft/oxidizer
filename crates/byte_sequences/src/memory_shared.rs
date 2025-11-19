// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::Memory;

/// Provides memory for byte sequences in a thread-safe manner.
///
/// This is a narrowing of [`Memory`] that adds additional constraints that enable
/// thread-safe shared access to the memory provider. If you do not need these extra
/// constraints, just use [`Memory`] directly.
pub trait MemoryShared: Memory + Send + Sync + 'static {}

impl<T> MemoryShared for T where T: Memory + Send + Sync + 'static {}
