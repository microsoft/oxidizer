// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::Debug;
use std::mem::MaybeUninit;

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Networking::WinSock::{SOCKET, closesocket};
use windows::Win32::Storage::FileSystem::SetFileCompletionNotificationModes;
use windows::Win32::System::IO::{
    CreateIoCompletionPort, OVERLAPPED_ENTRY, PostQueuedCompletionStatus,
};
use windows::core::{BOOL, Result};

use crate::pal::Bindings;

/// FFI bindings that target the real operating system that the build is targeting.
///
/// You would only use different bindings in PAL unit tests that need to use mock bindings.
/// Even then, whenever possible, unit tests should use real bindings for maximum realism.
#[derive(Debug, Default)]
pub struct BuildTargetBindings;

impl Bindings for BuildTargetBindings {
    #[cfg_attr(test, mutants::skip)] // Real PAL behavior is not meaningful to mutate, we try mutations manually via mock PAL.
    fn close_handle(&self, handle: HANDLE) -> Result<()> {
        // SAFETY: No safety requirements. Closing a handle twice is logically
        // invalid but does not violate Rust language rules, so not a safety concern.
        unsafe { CloseHandle(handle) }
    }

    #[cfg_attr(test, mutants::skip)] // Real PAL behavior is not meaningful to mutate, we try mutations manually via mock PAL.
    fn close_socket(&self, socket: SOCKET) -> i32 {
        // SAFETY: No safety requirements. Closing a socket twice is logically
        // invalid but does not violate Rust language rules, so not a safety concern.
        unsafe { closesocket(socket) }
    }

    #[cfg_attr(test, mutants::skip)] // Real PAL behavior is not meaningful to mutate, we try mutations manually via mock PAL.
    fn create_io_completion_port(
        &self,
        file_handle: HANDLE,
        existing_completion_port: Option<HANDLE>,
        completion_key: usize,
        number_of_concurrent_threads: u32,
    ) -> Result<HANDLE> {
        // SAFETY: No safety requirements.
        unsafe {
            CreateIoCompletionPort(
                file_handle,
                existing_completion_port,
                completion_key,
                number_of_concurrent_threads,
            )
        }
    }

    #[cfg_attr(test, mutants::skip)] // Real PAL behavior is not meaningful to mutate, we try mutations manually via mock PAL.
    unsafe fn set_file_completion_notification_mode(
        &self,
        file_handle: HANDLE,
        flags: u8,
    ) -> Result<()> {
        // SAFETY: We inherit and forward safety requirements from trait.
        unsafe { SetFileCompletionNotificationModes(file_handle, flags) }
    }

    #[cfg_attr(test, mutants::skip)] // Real PAL behavior is not meaningful to mutate, we try mutations manually via mock PAL.
    fn get_queued_completion_status_ex(
        &self,
        completion_port: HANDLE,
        completion_port_entries: &mut [MaybeUninit<OVERLAPPED_ENTRY>],
        num_entries_removed: &mut u32,
        milliseconds: u32,
        alertable: bool,
    ) -> Result<()> {
        let entries_len: u32 = completion_port_entries
            .len()
            .try_into()
            .expect("unrealistic that anyone will actually ask for more than u32 entries of data");

        // We use GetQueuedCompletionStatusEx from windows-sys because the one from the windows
        // crate takes `&mut` which presumes initialized memory. However, we are passing an
        // uninitialized buffer! https://github.com/microsoft/windows-rs/issues/2106
        #[expect(
            clippy::absolute_paths,
            reason = "intentionally being explicit for clarity"
        )]
        let entries_as_mut_ptr_sys = completion_port_entries
            .as_mut_ptr()
            .cast::<windows_sys::Win32::System::IO::OVERLAPPED_ENTRY>();

        // SAFETY: No safety requirements, the input pointers just have to outlive the call,
        // which they do, being on the stack until end of scope.
        let result_bool = unsafe {
            #[expect(
                clippy::absolute_paths,
                reason = "intentionally being explicit for clarity"
            )]
            windows_sys::Win32::System::IO::GetQueuedCompletionStatusEx(
                completion_port.0,
                entries_as_mut_ptr_sys,
                entries_len,
                &raw mut *num_entries_removed,
                milliseconds,
                windows_sys::Win32::Foundation::BOOL::from(alertable),
            )
        };

        BOOL(result_bool).ok()
    }

    #[cfg_attr(test, mutants::skip)] // Real PAL behavior is not meaningful to mutate, we try mutations manually via mock PAL.
    fn post_empty_queued_completion_status(
        &self,
        completion_port: HANDLE,
        completion_key: usize,
    ) -> Result<()> {
        // SAFETY: No safety requirements.
        unsafe { PostQueuedCompletionStatus(completion_port, 0, completion_key, None) }
    }
}