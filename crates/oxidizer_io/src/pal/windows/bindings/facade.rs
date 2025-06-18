// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::Debug;
use std::mem::MaybeUninit;
#[cfg(test)]
use std::sync::Arc;

use windows::Win32::Foundation::HANDLE;
use windows::Win32::Networking::WinSock::SOCKET;
use windows::Win32::System::IO::OVERLAPPED_ENTRY;
use windows::core::Result;

#[cfg(test)]
use crate::pal::MockBindings;
use crate::pal::{Bindings, BuildTargetBindings};

// Hides the difference between mock and real bindings behind a common facade.
#[derive(Clone, Debug)]
pub enum BindingsFacade {
    Real(&'static BuildTargetBindings),

    #[cfg(test)]
    Mock(Arc<MockBindings>),
}

impl BindingsFacade {
    #[cfg_attr(test, mutants::skip)] // Low-impact layer, waste of time to mutate.
    pub const fn real() -> Self {
        Self::Real(&BuildTargetBindings)
    }

    #[cfg(test)]
    #[cfg_attr(test, mutants::skip)] // Low-impact layer, waste of time to mutate.
    pub fn from_mock(bindings: MockBindings) -> Self {
        Self::Mock(Arc::new(bindings))
    }
}

impl Bindings for BindingsFacade {
    #[cfg_attr(test, mutants::skip)] // Low-impact layer, waste of time to mutate.
    fn close_handle(&self, handle: HANDLE) -> Result<()> {
        match self {
            Self::Real(real) => real.close_handle(handle),
            #[cfg(test)]
            Self::Mock(mock) => mock.close_handle(handle),
        }
    }

    #[cfg_attr(test, mutants::skip)] // Low-impact layer, waste of time to mutate.
    fn close_socket(&self, socket: SOCKET) -> i32 {
        match self {
            Self::Real(real) => real.close_socket(socket),
            #[cfg(test)]
            Self::Mock(mock) => mock.close_socket(socket),
        }
    }

    #[cfg_attr(test, mutants::skip)] // Low-impact layer, waste of time to mutate.
    fn create_io_completion_port(
        &self,
        file_handle: HANDLE,
        existing_completion_port: Option<HANDLE>,
        completion_key: usize,
        number_of_concurrent_threads: u32,
    ) -> Result<HANDLE> {
        match self {
            Self::Real(real) => real.create_io_completion_port(
                file_handle,
                existing_completion_port,
                completion_key,
                number_of_concurrent_threads,
            ),
            #[cfg(test)]
            Self::Mock(mock) => mock.create_io_completion_port(
                file_handle,
                existing_completion_port,
                completion_key,
                number_of_concurrent_threads,
            ),
        }
    }

    #[cfg_attr(test, mutants::skip)] // Low-impact layer, waste of time to mutate.
    unsafe fn set_file_completion_notification_mode(
        &self,
        file_handle: HANDLE,
        flags: u8,
    ) -> Result<()> {
        match self {
            Self::Real(real) => {
                // SAFETY: Forwarding safety requirements.
                unsafe { real.set_file_completion_notification_mode(file_handle, flags) }
            }
            #[cfg(test)]
            Self::Mock(mock) => {
                // SAFETY: Forwarding safety requirements.
                unsafe { mock.set_file_completion_notification_mode(file_handle, flags) }
            }
        }
    }

    #[cfg_attr(test, mutants::skip)] // Low-impact layer, waste of time to mutate.
    fn get_queued_completion_status_ex(
        &self,
        completion_port: HANDLE,
        completion_port_entries: &mut [MaybeUninit<OVERLAPPED_ENTRY>],
        num_entries_removed: &mut u32,
        milliseconds: u32,
        alertable: bool,
    ) -> Result<()> {
        match self {
            Self::Real(real) => real.get_queued_completion_status_ex(
                completion_port,
                completion_port_entries,
                num_entries_removed,
                milliseconds,
                alertable,
            ),
            #[cfg(test)]
            Self::Mock(mock) => mock.get_queued_completion_status_ex(
                completion_port,
                completion_port_entries,
                num_entries_removed,
                milliseconds,
                alertable,
            ),
        }
    }

    #[cfg_attr(test, mutants::skip)] // Low-impact layer, waste of time to mutate.
    fn post_empty_queued_completion_status(
        &self,
        completion_port: HANDLE,
        completion_key: usize,
    ) -> Result<()> {
        match self {
            Self::Real(real) => {
                real.post_empty_queued_completion_status(completion_port, completion_key)
            }
            #[cfg(test)]
            Self::Mock(mock) => {
                mock.post_empty_queued_completion_status(completion_port, completion_key)
            }
        }
    }
}