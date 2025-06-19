// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use windows::Win32::Foundation::ERROR_IO_PENDING;
use windows::core::BOOL;

use crate::BeginResult;

impl BeginResult<()> {
    #[must_use]
    pub fn from_bool(success: BOOL) -> Self {
        Self::from_windows_result(success.ok())
    }

    #[must_use]
    pub fn from_windows_result(result: windows::core::Result<()>) -> Self {
        match result {
            Ok(()) => Self::CompletedSynchronously(Ok(())),
            // This just means "asynchronous operation started", not a real error.
            Err(e) if e.code() == ERROR_IO_PENDING.into() => Self::Asynchronous,
            // A real error - something is wrong, the call failed.
            Err(e) => Self::CompletedSynchronously(Err(crate::Error::Windows(e))),
        }
    }
}