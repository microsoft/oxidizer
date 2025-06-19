// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::Debug;

use crate::pal::ElementaryOperationKey;

/// Metadata about an asynchronous elementary I/O operation that has been signaled completed.
pub trait CompletionNotification: Debug {
    /// Whether this is a wake-up signal that points to a non-existing elementary operation.
    fn is_wake_up_signal(&self) -> bool;

    /// The operating system uses pointers to identify elementary I/O operations. It is up to the
    /// caller to figure out a safe way to get from the pointer to the I/O operation instance.
    fn elementary_operation_key(&self) -> ElementaryOperationKey;

    /// The result of the operation, indicating number of bytes transferred on success.
    fn result(&self) -> crate::Result<u32>;
}