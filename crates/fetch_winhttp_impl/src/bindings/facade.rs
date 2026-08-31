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

/// Forwards one `Bindings` method to the installed variant without changing
/// arguments or lifecycle semantics.
///
/// Generated once per method so a transposed argument is a type error at the
/// call site rather than a silent mismatch buried in a hand-written match arm.
macro_rules! forward_bindings {
    ($(unsafe fn $name:ident($($arg:ident: $ty:ty),* $(,)?) -> $ret:ty;)*) => {
        $(
            unsafe fn $name(&self, $($arg: $ty),*) -> $ret {
                match self {
                    Self::Real => {
                        // SAFETY: the caller contract is forwarded unchanged.
                        unsafe { RealBindings.$name($($arg),*) }
                    }
                    #[cfg(test)]
                    Self::Mock(bindings) => {
                        // SAFETY: the caller contract is forwarded unchanged.
                        unsafe { bindings.$name($($arg),*) }
                    }
                }
            }
        )*
    };
}

// SAFETY: Each branch forwards every operation without changing its arguments,
// callback behavior, or lifecycle semantics. Real bindings preserve WinHTTP
// behavior, while mock bindings are configured by tests to model the same
// handle and completion protocol.
unsafe impl Bindings for BindingsFacade {
    forward_bindings! {
        unsafe fn open(user_agent: &U16CStr, flags: u32) -> Result<RawHandle>;
        unsafe fn set_timeouts(handle: RawHandle, resolve: i32, connect: i32, send: i32, receive: i32) -> Result<()>;
        unsafe fn set_status_callback(handle: RawHandle, callback: StatusCallback, notification_flags: u32) -> Result<()>;
        unsafe fn connect(session: RawHandle, host: &U16CStr, port: u16) -> Result<RawHandle>;
        unsafe fn open_request(connect: RawHandle, method: &U16CStr, path: &U16CStr, flags: u32) -> Result<RawHandle>;
        unsafe fn set_option(handle: RawHandle, option: u32, value: &[u8]) -> Result<()>;
        unsafe fn send_request(request: RawHandle, headers: &U16CStr, total_len: u32, context: usize) -> Result<()>;
        unsafe fn write_data(request: RawHandle, buffer: Option<NonNull<u8>>, len: u32) -> Result<()>;
        unsafe fn receive_response(request: RawHandle) -> Result<()>;
        unsafe fn query_headers(request: RawHandle, info_level: u32, buffer: Option<NonNull<u8>>, buffer_len: &mut u32) -> Result<()>;
        unsafe fn query_option(handle: RawHandle, option: u32, buffer: Option<NonNull<u8>>, buffer_len: &mut u32) -> Result<()>;
        unsafe fn read_data_ex(request: RawHandle, buffer: NonNull<u8>, len: u32, fill_buffer: bool) -> Result<()>;
        unsafe fn close_handle(handle: RawHandle) -> Result<()>;
    }
}
