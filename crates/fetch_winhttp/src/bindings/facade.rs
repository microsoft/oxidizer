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
/// Dispatches every WinHTTP call to the production bindings or to a test double.
///
/// This is the crate's operating-system abstraction: session, request, and body
/// logic reach WinHTTP only through this type, which is what lets the transport
/// be exercised with no network, no real handle, and no real OS behavior
/// (implementation.md, "The bindings facade").
///
/// Each RAII handle wrapper (`SessionHandle`, `ConnectHandle`, `RequestHandle`)
/// embeds one by value so it can close its handle on drop, so a value is cloned
/// for every handle a request constructs. Both variants stay cheap to clone for
/// that reason: `Real` carries no state and `Mock` clones a single `Arc`. The
/// `Mock` variant is compiled only under `cfg(test)`, so production builds
/// dispatch through a single-variant enum.
///
/// Dispatch forwards arguments, callback registration, and handle lifecycle
/// unchanged, so the [`Bindings`] contract, including its safety requirements,
/// applies identically whichever variant is installed, and callers never need
/// to know which one that is.
pub(crate) enum BindingsFacade {
    Real,
    #[cfg(test)]
    Mock(Arc<MockBindings>),
}

impl BindingsFacade {
    pub(crate) const fn real() -> Self {
        Self::Real
    }

    #[cfg(test)]
    pub(crate) fn mock(bindings: Arc<MockBindings>) -> Self {
        Self::Mock(bindings)
    }
}

impl fmt::Debug for BindingsFacade {
    #[cfg_attr(coverage_nightly, coverage(off))] // We have no API contract here.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Real => f.write_str("BindingsFacade::Real"),
            #[cfg(test)]
            Self::Mock(_) => f.write_str("BindingsFacade::Mock"),
        }
    }
}

// SAFETY: Each branch forwards every operation without changing its arguments,
// callback behavior, or lifecycle semantics. Real bindings preserve WinHTTP
// behavior, while mock bindings are configured by tests to model the same
// handle and completion protocol.
unsafe impl Bindings for BindingsFacade {
    unsafe fn open(&self, user_agent: &U16CStr, flags: u32) -> Result<RawHandle> {
        match self {
            Self::Real => {
                // SAFETY: the caller contract is forwarded unchanged.
                unsafe { RealBindings.open(user_agent, flags) }
            }
            #[cfg(test)]
            Self::Mock(bindings) => {
                // SAFETY: the caller contract is forwarded unchanged.
                unsafe { bindings.open(user_agent, flags) }
            }
        }
    }

    unsafe fn set_timeouts(&self, handle: RawHandle, resolve: i32, connect: i32, send: i32, receive: i32) -> Result<()> {
        match self {
            Self::Real => {
                // SAFETY: the caller contract is forwarded unchanged.
                unsafe { RealBindings.set_timeouts(handle, resolve, connect, send, receive) }
            }
            #[cfg(test)]
            Self::Mock(bindings) => {
                // SAFETY: the caller contract is forwarded unchanged.
                unsafe { bindings.set_timeouts(handle, resolve, connect, send, receive) }
            }
        }
    }

    unsafe fn set_status_callback(&self, handle: RawHandle, callback: StatusCallback, notification_flags: u32) -> Result<()> {
        match self {
            Self::Real => {
                // SAFETY: the caller contract is forwarded unchanged.
                unsafe { RealBindings.set_status_callback(handle, callback, notification_flags) }
            }
            #[cfg(test)]
            Self::Mock(bindings) => {
                // SAFETY: the caller contract is forwarded unchanged.
                unsafe { bindings.set_status_callback(handle, callback, notification_flags) }
            }
        }
    }

    unsafe fn connect(&self, session: RawHandle, host: &U16CStr, port: u16) -> Result<RawHandle> {
        match self {
            Self::Real => {
                // SAFETY: the caller contract is forwarded unchanged.
                unsafe { RealBindings.connect(session, host, port) }
            }
            #[cfg(test)]
            Self::Mock(bindings) => {
                // SAFETY: the caller contract is forwarded unchanged.
                unsafe { bindings.connect(session, host, port) }
            }
        }
    }

    unsafe fn open_request(&self, connect: RawHandle, method: &U16CStr, path: &U16CStr, flags: u32) -> Result<RawHandle> {
        match self {
            Self::Real => {
                // SAFETY: the caller contract is forwarded unchanged.
                unsafe { RealBindings.open_request(connect, method, path, flags) }
            }
            #[cfg(test)]
            Self::Mock(bindings) => {
                // SAFETY: the caller contract is forwarded unchanged.
                unsafe { bindings.open_request(connect, method, path, flags) }
            }
        }
    }

    unsafe fn set_option(&self, handle: RawHandle, option: u32, value: &[u8]) -> Result<()> {
        match self {
            Self::Real => {
                // SAFETY: the caller contract is forwarded unchanged.
                unsafe { RealBindings.set_option(handle, option, value) }
            }
            #[cfg(test)]
            Self::Mock(bindings) => {
                // SAFETY: the caller contract is forwarded unchanged.
                unsafe { bindings.set_option(handle, option, value) }
            }
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

    unsafe fn receive_response(&self, request: RawHandle) -> Result<()> {
        match self {
            Self::Real => {
                // SAFETY: the caller contract is forwarded unchanged.
                unsafe { RealBindings.receive_response(request) }
            }
            #[cfg(test)]
            Self::Mock(bindings) => {
                // SAFETY: the caller contract is forwarded unchanged.
                unsafe { bindings.receive_response(request) }
            }
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

    unsafe fn query_data_available(&self, request: RawHandle) -> Result<()> {
        match self {
            Self::Real => {
                // SAFETY: the caller contract is forwarded unchanged.
                unsafe { RealBindings.query_data_available(request) }
            }
            #[cfg(test)]
            Self::Mock(bindings) => {
                // SAFETY: the caller contract is forwarded unchanged.
                unsafe { bindings.query_data_available(request) }
            }
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

    unsafe fn close_handle(&self, handle: RawHandle) -> Result<()> {
        match self {
            Self::Real => {
                // SAFETY: the caller contract is forwarded unchanged.
                unsafe { RealBindings.close_handle(handle) }
            }
            #[cfg(test)]
            Self::Mock(bindings) => {
                // SAFETY: the caller contract is forwarded unchanged.
                unsafe { bindings.close_handle(handle) }
            }
        }
    }
}
