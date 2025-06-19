// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use windows::Win32::Foundation::{NTSTATUS, STATUS_SUCCESS};
use windows::Win32::System::IO::OVERLAPPED_ENTRY;

use crate::pal::{CompletionNotification, ElementaryOperationKey};
use crate::thread_safe::ThreadSafe;

#[derive(Clone, Copy, Debug)]
#[repr(transparent)]
pub struct CompletionNotificationImpl {
    // In its natural form, this is not thread-safe because it contains a raw pointer. However,
    // we are using it with all proper precautions, so we simply assert thread safety here.
    inner: ThreadSafe<OVERLAPPED_ENTRY>,
}

impl CompletionNotification for CompletionNotificationImpl {
    fn elementary_operation_key(&self) -> ElementaryOperationKey {
        // For the Windows PAL, the elementary operation key is a pointer to the OVERLAPPED.
        ElementaryOperationKey(self.inner.lpOverlapped as usize)
    }

    fn result(&self) -> crate::Result<u32> {
        #[expect(
            clippy::cast_possible_wrap,
            clippy::cast_possible_truncation,
            reason = "Win32 API says this is okay"
        )]
        let status = NTSTATUS(self.inner.Internal as i32);

        if status == STATUS_SUCCESS {
            let bytes_transferred = self.inner.dwNumberOfBytesTransferred;
            Ok(bytes_transferred)
        } else {
            Err(crate::Error::Windows(status.into()))
        }
    }

    fn is_wake_up_signal(&self) -> bool {
        self.inner.lpCompletionKey == super::constants::WAKE_UP_COMPLETION_KEY
    }
}

#[cfg(test)]
mod tests {
    use std::mem;

    use static_assertions::const_assert_eq;

    use super::*;

    #[test]
    const fn is_overlapped_entry() {
        const_assert_eq!(
            mem::size_of::<CompletionNotificationImpl>(),
            mem::size_of::<OVERLAPPED_ENTRY>()
        );
        const_assert_eq!(
            mem::align_of::<CompletionNotificationImpl>(),
            mem::align_of::<OVERLAPPED_ENTRY>()
        );
    }
}