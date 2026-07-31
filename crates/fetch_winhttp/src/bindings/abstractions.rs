// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::ffi::c_void;
use std::ptr::NonNull;

use widestring::U16CStr;

use crate::error::Result;
use crate::handle::RawHandle;

/// Callback ABI accepted by WinHTTP, with `None` clearing the callback.
pub(crate) type StatusCallback = Option<unsafe extern "system" fn(*mut c_void, usize, u32, *mut c_void, u32)>;

/// Defines the OS boundary used by the transport and its deterministic tests.
///
/// Implementations provide only the WinHTTP operations the transport needs.
/// Callers remain responsible for the handle, context, and asynchronous buffer
/// lifetime contracts stated on the unsafe methods.
#[cfg_attr(test, mockall::automock)]
pub(crate) trait Bindings: Send + Sync + 'static {
    fn open(&self, user_agent: &U16CStr, flags: u32) -> Result<RawHandle>;

    fn set_timeouts(&self, handle: RawHandle, resolve: i32, connect: i32, send: i32, receive: i32) -> Result<()>;

    fn set_status_callback(&self, handle: RawHandle, callback: StatusCallback, notification_flags: u32) -> Result<()>;

    fn connect(&self, session: RawHandle, host: &U16CStr, port: u16) -> Result<RawHandle>;

    fn open_request(&self, connect: RawHandle, method: &U16CStr, path: &U16CStr, flags: u32) -> Result<RawHandle>;

    fn set_option(&self, handle: RawHandle, option: u32, value: &[u8]) -> Result<()>;

    /// # Safety
    ///
    /// The request context must remain valid until the request handle's final
    /// `HANDLE_CLOSING` callback.
    unsafe fn send_request(&self, request: RawHandle, headers: &U16CStr, total_len: u32, context: usize) -> Result<()>;

    /// # Safety
    ///
    /// A present `buffer` must remain valid and unchanged until
    /// `WRITE_COMPLETE`, `REQUEST_ERROR`, or the request handle's final
    /// `HANDLE_CLOSING` callback terminates the operation. An absent buffer is
    /// valid only for the zero-length write that ends automatic chunking.
    unsafe fn write_data(&self, request: RawHandle, buffer: Option<NonNull<u8>>, len: u32) -> Result<()>;

    fn receive_response(&self, request: RawHandle) -> Result<()>;

    /// # Safety
    ///
    /// `buffer` must be writable for `*buffer_len` bytes when present.
    unsafe fn query_headers(&self, request: RawHandle, info_level: u32, buffer: Option<NonNull<u8>>, buffer_len: &mut u32) -> Result<()>;

    /// # Safety
    ///
    /// `buffer` must be writable for `*buffer_len` bytes when present.
    unsafe fn query_option(&self, handle: RawHandle, option: u32, buffer: Option<NonNull<u8>>, buffer_len: &mut u32) -> Result<()>;

    fn query_data_available(&self, request: RawHandle) -> Result<()>;

    /// # Safety
    ///
    /// `buffer` must remain valid, writable, and untouched until
    /// `READ_COMPLETE`, `REQUEST_ERROR`, or the request handle's final
    /// `HANDLE_CLOSING` callback terminates the operation.
    unsafe fn read_data(&self, request: RawHandle, buffer: NonNull<u8>, len: u32) -> Result<()>;

    fn close_handle(&self, handle: RawHandle) -> Result<()>;
}
