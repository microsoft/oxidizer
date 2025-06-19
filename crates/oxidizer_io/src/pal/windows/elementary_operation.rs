// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::Debug;
use std::marker::PhantomPinned;
use std::pin::Pin;
use std::ptr;

use windows::Win32::System::IO::OVERLAPPED;

use crate::pal::{ElementaryOperation, ElementaryOperationKey};
use crate::thread_safe::ThreadSafe;

#[repr(transparent)]
pub struct ElementaryOperationImpl {
    // This is not thread-safe in its natural form because it contains a raw pointer. However,
    // we are using it with all proper precautions, so we simply assert thread safety here.
    overlapped: ThreadSafe<OVERLAPPED>,

    // Instances of this type must be pinned in memory during use.
    _requires_pinning: PhantomPinned,
}

impl ElementaryOperationImpl {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "Splitting a value in two parts, no truncation"
    )]
    pub(crate) fn new(offset: u64) -> Self {
        let mut overlapped = OVERLAPPED::default();
        overlapped.Anonymous.Anonymous.Offset = offset as u32;
        overlapped.Anonymous.Anonymous.OffsetHigh = (offset >> 32) as u32;

        Self {
            // SAFETY: We must promise it really is used in a thread-safe way. Yes, it is.
            // The lack of natural thread safety is just because it contains raw pointers.
            overlapped: unsafe { ThreadSafe::new(overlapped) },
            _requires_pinning: PhantomPinned,
        }
    }
}

impl ElementaryOperation for ElementaryOperationImpl {
    fn key(self: Pin<&Self>) -> ElementaryOperationKey {
        // We guarantee that the key is a pointer to the OVERLAPPED structure. This is how
        // Windows-specific code in the upper layers extracts the OVERLAPPED from the operation.
        ElementaryOperationKey(ptr::from_ref(&self.overlapped).expose_provenance())
    }
}

impl Debug for ElementaryOperationImpl {
    #[cfg_attr(test, mutants::skip)] // There is no API contract this needs to satisfy.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ElementaryOperation")
            .field("overlapped", &format_args!("{:p}", &self.overlapped))
            .finish_non_exhaustive()
    }
}