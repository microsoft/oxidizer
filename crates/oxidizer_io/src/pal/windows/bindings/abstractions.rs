// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::Debug;
use std::mem::MaybeUninit;

use windows::Win32::Foundation::HANDLE;
use windows::Win32::Networking::WinSock::SOCKET;
use windows::Win32::System::IO::OVERLAPPED_ENTRY;
use windows::core::Result;

/// Bindings for FFI calls into external libraries (either provided by operating system or not).
///
/// All PAL FFI calls must go through this trait, enabling them to be mocked.
#[cfg_attr(test, mockall::automock)]
pub trait Bindings: Debug + Send + Sync + 'static {
    fn close_handle(&self, handle: HANDLE) -> Result<()>;
    fn close_socket(&self, socket: SOCKET) -> i32;

    fn create_io_completion_port(
        &self,
        file_handle: HANDLE,
        existing_completion_port: Option<HANDLE>,
        completion_key: usize,
        number_of_concurrent_threads: u32,
    ) -> Result<HANDLE>;

    /// # Safety
    ///
    /// Understand the impact of the flags you set, as they may affect resource management logic
    /// related to elementary I/O operations.
    unsafe fn set_file_completion_notification_mode(
        &self,
        file_handle: HANDLE,
        flags: u8,
    ) -> Result<()>;

    fn get_queued_completion_status_ex(
        &self,
        completion_port: HANDLE,
        completion_port_entries: &mut [MaybeUninit<OVERLAPPED_ENTRY>],
        num_entries_removed: &mut u32,
        milliseconds: u32,
        alertable: bool,
    ) -> Result<()>;

    // empty == no bytes transferred, no overlapped structure
    fn post_empty_queued_completion_status(
        &self,
        completion_port: HANDLE,
        completion_key: usize,
    ) -> Result<()>;
}