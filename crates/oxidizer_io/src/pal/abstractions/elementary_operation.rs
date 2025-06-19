// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::Debug;
use std::pin::Pin;

/// An elementary I/O operation issued to the operating system (equivalent to one syscall).
///
/// Instances of these are co-owned by the operating system when they are in progress. We
/// communicate with the operating system about these operations by passing pointers to them
/// and receiving back the pointers when the operation has been completed.
pub trait ElementaryOperation: Debug {
    /// Used to reference this elementary operation in completion notifications.
    fn key(self: Pin<&Self>) -> ElementaryOperationKey;
}

/// Platform-specific contents packaged as a tuple struct just to avoid accidental conversions.
///
/// In a real implementation, this might be a pointer; in a mock implementation,
/// this might be an index into some test data. Read platform-specific documentation to learn more.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ElementaryOperationKey(pub usize);