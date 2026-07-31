// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt;
use std::ptr::NonNull;
#[cfg(test)]
use std::sync::Arc;

use widestring::U16CStr;

#[cfg(test)]
use super::MockBindings;
use super::real::RealBindings;
use super::{Bindings, StatusCallback};
use crate::error::Result;
use crate::handle::RawHandle;

#[derive(Clone)]
pub(crate) enum Facade {
    Real,
    #[cfg(test)]
    Mock(Arc<MockBindings>),
}

impl Facade {
    pub(crate) const fn real() -> Self {
        Self::Real
    }

    #[cfg(test)]
    pub(crate) fn mock(bindings: Arc<MockBindings>) -> Self {
        Self::Mock(bindings)
    }
}

impl Default for Facade {
    fn default() -> Self {
        Self::real()
    }
}

impl fmt::Debug for Facade {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Real => f.write_str("Facade::Real"),
            #[cfg(test)]
            Self::Mock(_) => f.write_str("Facade::Mock"),
        }
    }
}

impl Bindings for Facade {
    fn open(&self, user_agent: &U16CStr, flags: u32) -> Result<RawHandle> {
        match self {
            Self::Real => RealBindings.open(user_agent, flags),
            #[cfg(test)]
            Self::Mock(bindings) => bindings.open(user_agent, flags),
        }
    }

    fn set_timeouts(&self, handle: RawHandle, resolve: i32, connect: i32, send: i32, receive: i32) -> Result<()> {
        match self {
            Self::Real => RealBindings.set_timeouts(handle, resolve, connect, send, receive),
            #[cfg(test)]
            Self::Mock(bindings) => bindings.set_timeouts(handle, resolve, connect, send, receive),
        }
    }

    fn set_status_callback(&self, handle: RawHandle, callback: StatusCallback, notification_flags: u32) -> Result<()> {
        match self {
            Self::Real => RealBindings.set_status_callback(handle, callback, notification_flags),
            #[cfg(test)]
            Self::Mock(bindings) => bindings.set_status_callback(handle, callback, notification_flags),
        }
    }

    fn connect(&self, session: RawHandle, host: &U16CStr, port: u16) -> Result<RawHandle> {
        match self {
            Self::Real => RealBindings.connect(session, host, port),
            #[cfg(test)]
            Self::Mock(bindings) => bindings.connect(session, host, port),
        }
    }

    fn open_request(&self, connect: RawHandle, method: &U16CStr, path: &U16CStr, flags: u32) -> Result<RawHandle> {
        match self {
            Self::Real => RealBindings.open_request(connect, method, path, flags),
            #[cfg(test)]
            Self::Mock(bindings) => bindings.open_request(connect, method, path, flags),
        }
    }

    fn set_option(&self, handle: RawHandle, option: u32, value: &[u8]) -> Result<()> {
        match self {
            Self::Real => RealBindings.set_option(handle, option, value),
            #[cfg(test)]
            Self::Mock(bindings) => bindings.set_option(handle, option, value),
        }
    }

    unsafe fn send_request(&self, request: RawHandle, headers: &U16CStr, total_len: u32, context: usize) -> Result<()> {
        match self {
            Self::Real => {
                // SAFETY: the caller contract is forwarded unchanged.
                unsafe { RealBindings.send_request(request, headers, total_len, context) }
            }
            #[cfg(test)]
            Self::Mock(bindings) => {
                // SAFETY: the caller contract is forwarded unchanged.
                unsafe { bindings.send_request(request, headers, total_len, context) }
            }
        }
    }

    unsafe fn write_data(&self, request: RawHandle, buffer: Option<NonNull<u8>>, len: u32) -> Result<()> {
        match self {
            Self::Real => {
                // SAFETY: the caller contract is forwarded unchanged.
                unsafe { RealBindings.write_data(request, buffer, len) }
            }
            #[cfg(test)]
            Self::Mock(bindings) => {
                // SAFETY: the caller contract is forwarded unchanged.
                unsafe { bindings.write_data(request, buffer, len) }
            }
        }
    }

    fn receive_response(&self, request: RawHandle) -> Result<()> {
        match self {
            Self::Real => RealBindings.receive_response(request),
            #[cfg(test)]
            Self::Mock(bindings) => bindings.receive_response(request),
        }
    }

    unsafe fn query_headers(&self, request: RawHandle, info_level: u32, buffer: Option<NonNull<u8>>, buffer_len: &mut u32) -> Result<()> {
        match self {
            Self::Real => {
                // SAFETY: the caller contract is forwarded unchanged.
                unsafe { RealBindings.query_headers(request, info_level, buffer, buffer_len) }
            }
            #[cfg(test)]
            Self::Mock(bindings) => {
                // SAFETY: the caller contract is forwarded unchanged.
                unsafe { bindings.query_headers(request, info_level, buffer, buffer_len) }
            }
        }
    }

    unsafe fn query_option(&self, handle: RawHandle, option: u32, buffer: Option<NonNull<u8>>, buffer_len: &mut u32) -> Result<()> {
        match self {
            Self::Real => {
                // SAFETY: the caller contract is forwarded unchanged.
                unsafe { RealBindings.query_option(handle, option, buffer, buffer_len) }
            }
            #[cfg(test)]
            Self::Mock(bindings) => {
                // SAFETY: the caller contract is forwarded unchanged.
                unsafe { bindings.query_option(handle, option, buffer, buffer_len) }
            }
        }
    }

    fn query_data_available(&self, request: RawHandle) -> Result<()> {
        match self {
            Self::Real => RealBindings.query_data_available(request),
            #[cfg(test)]
            Self::Mock(bindings) => bindings.query_data_available(request),
        }
    }

    unsafe fn read_data(&self, request: RawHandle, buffer: NonNull<u8>, len: u32) -> Result<()> {
        match self {
            Self::Real => {
                // SAFETY: the caller contract is forwarded unchanged.
                unsafe { RealBindings.read_data(request, buffer, len) }
            }
            #[cfg(test)]
            Self::Mock(bindings) => {
                // SAFETY: the caller contract is forwarded unchanged.
                unsafe { bindings.read_data(request, buffer, len) }
            }
        }
    }

    fn close_handle(&self, handle: RawHandle) -> Result<()> {
        match self {
            Self::Real => RealBindings.close_handle(handle),
            #[cfg(test)]
            Self::Mock(bindings) => bindings.close_handle(handle),
        }
    }
}
