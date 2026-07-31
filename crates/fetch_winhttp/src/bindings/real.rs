// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::ffi::c_void;
use std::ptr::{NonNull, null, null_mut};

use widestring::U16CStr;
use windows::Win32::Foundation::GetLastError;
use windows::Win32::Networking::WinHttp::{
    WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_OPEN_REQUEST_FLAGS, WinHttpCloseHandle, WinHttpConnect, WinHttpOpen, WinHttpOpenRequest,
    WinHttpQueryDataAvailable, WinHttpQueryHeaders, WinHttpQueryOption, WinHttpReadData, WinHttpReceiveResponse, WinHttpSendRequest,
    WinHttpSetOption, WinHttpSetStatusCallback, WinHttpSetTimeouts, WinHttpWriteData,
};
use windows::core::{Error as WindowsError, PCWSTR};

use super::{Bindings, StatusCallback};
use crate::error::{Result, WinHttpError, WinHttpOperation};
use crate::handle::RawHandle;

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct RealBindings;

impl RealBindings {
    fn error_from_last_error(operation: WinHttpOperation) -> WinHttpError {
        // SAFETY: GetLastError has no preconditions and is called immediately
        // after the WinHTTP function reported failure.
        let code = unsafe { GetLastError().0 };

        WinHttpError::new(code, operation)
    }

    fn map_error(error: &WindowsError, operation: WinHttpOperation) -> WinHttpError {
        WinHttpError::from_hresult(error.code().0, operation)
    }

    fn handle_result(handle: *mut c_void, operation: WinHttpOperation) -> Result<RawHandle> {
        Self::handle_result_with(handle, || Self::error_from_last_error(operation))
    }

    fn handle_result_with(handle: *mut c_void, error: impl FnOnce() -> WinHttpError) -> Result<RawHandle> {
        RawHandle::new(handle).ok_or_else(error)
    }

    fn callback_result(callback_address: Option<usize>, operation: WinHttpOperation) -> Result<()> {
        Self::callback_result_with(callback_address, || Self::error_from_last_error(operation))
    }

    fn callback_result_with(callback_address: Option<usize>, error: impl FnOnce() -> WinHttpError) -> Result<()> {
        if callback_address == Some(usize::MAX) {
            Err(error())
        } else {
            Ok(())
        }
    }
}

impl Bindings for RealBindings {
    fn open(&self, user_agent: &U16CStr, flags: u32) -> Result<RawHandle> {
        // SAFETY: all string pointers are NUL-terminated for the duration of
        // the call; proxy and bypass are intentionally null.
        let handle = unsafe {
            WinHttpOpen(
                PCWSTR(user_agent.as_ptr()),
                WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
                PCWSTR::null(),
                PCWSTR::null(),
                flags,
            )
        };

        Self::handle_result(handle, WinHttpOperation::Open)
    }

    fn set_timeouts(&self, handle: RawHandle, resolve: i32, connect: i32, send: i32, receive: i32) -> Result<()> {
        // SAFETY: the typed handle is non-null and the timeout values use the
        // exact WinHttpSetTimeouts ABI types.
        unsafe { WinHttpSetTimeouts(handle.as_ptr(), resolve, connect, send, receive) }
            .map_err(|error| Self::map_error(&error, WinHttpOperation::SetTimeouts))
    }

    #[expect(
        clippy::fn_to_numeric_cast_any,
        reason = "the WinHTTP failure sentinel is the function-pointer bit pattern usize::MAX"
    )]
    fn set_status_callback(&self, handle: RawHandle, callback: StatusCallback, notification_flags: u32) -> Result<()> {
        // SAFETY: the typed handle is non-null, the callback has the required
        // system ABI, and the reserved parameter is zero as required.
        let previous = unsafe { WinHttpSetStatusCallback(handle.as_ptr(), callback, notification_flags, 0) };

        Self::callback_result(previous.map(|callback| callback as usize), WinHttpOperation::SetStatusCallback)
    }

    fn connect(&self, session: RawHandle, host: &U16CStr, port: u16) -> Result<RawHandle> {
        // SAFETY: the session is non-null and host is NUL-terminated for the
        // duration of the call; the reserved value is zero.
        let handle = unsafe { WinHttpConnect(session.as_ptr(), PCWSTR(host.as_ptr()), port, 0) };

        Self::handle_result(handle, WinHttpOperation::Connect)
    }

    fn open_request(&self, connect: RawHandle, method: &U16CStr, path: &U16CStr, flags: u32) -> Result<RawHandle> {
        // SAFETY: the handle is non-null, method and path are NUL-terminated,
        // optional strings are null, and the accept-type list is null.
        let handle = unsafe {
            WinHttpOpenRequest(
                connect.as_ptr(),
                PCWSTR(method.as_ptr()),
                PCWSTR(path.as_ptr()),
                PCWSTR::null(),
                PCWSTR::null(),
                null(),
                WINHTTP_OPEN_REQUEST_FLAGS(flags),
            )
        };

        Self::handle_result(handle, WinHttpOperation::OpenRequest)
    }

    fn set_option(&self, handle: RawHandle, option: u32, value: &[u8]) -> Result<()> {
        // SAFETY: the handle is non-null and the byte slice remains valid for
        // the synchronous option-setting call.
        unsafe { WinHttpSetOption(Some(handle.as_const_ptr()), option, Some(value)) }
            .map_err(|error| Self::map_error(&error, WinHttpOperation::SetOption))
    }

    unsafe fn send_request(&self, request: RawHandle, headers: &U16CStr, total_len: u32, context: usize) -> Result<()> {
        // SAFETY: the caller upholds the request-context lifetime contract.
        // U16CStr::as_slice excludes the trailing NUL from the header length;
        // request bodies are submitted through sequential WinHttpWriteData
        // calls, so the optional buffer is intentionally null.
        unsafe { WinHttpSendRequest(request.as_ptr(), Some(headers.as_slice()), None, 0, total_len, context) }
            .map_err(|error| Self::map_error(&error, WinHttpOperation::SendRequest))
    }

    unsafe fn write_data(&self, request: RawHandle, buffer: Option<NonNull<u8>>, len: u32) -> Result<()> {
        debug_assert!(buffer.is_some() || len == 0, "only the final zero-length write may omit its buffer");
        let buffer = buffer.map(|buffer| buffer.as_ptr().cast_const().cast());

        // SAFETY: the caller upholds the asynchronous buffer-lifetime
        // contract; a null buffer is paired only with zero length, and the
        // asynchronous OUT pointer is intentionally null.
        unsafe { WinHttpWriteData(request.as_ptr(), buffer, len, null_mut()) }
            .map_err(|error| Self::map_error(&error, WinHttpOperation::WriteData))
    }

    fn receive_response(&self, request: RawHandle) -> Result<()> {
        // SAFETY: the request handle is non-null and the reserved pointer is
        // required to be null.
        unsafe { WinHttpReceiveResponse(request.as_ptr(), null_mut()) }
            .map_err(|error| Self::map_error(&error, WinHttpOperation::ReceiveResponse))
    }

    unsafe fn query_headers(&self, request: RawHandle, info_level: u32, buffer: Option<NonNull<u8>>, buffer_len: &mut u32) -> Result<()> {
        let buffer = buffer.map(|pointer| pointer.as_ptr().cast());

        // SAFETY: the caller guarantees the optional output buffer capacity;
        // header name and index are intentionally null.
        unsafe { WinHttpQueryHeaders(request.as_ptr(), info_level, PCWSTR::null(), buffer, buffer_len, null_mut()) }
            .map_err(|error| Self::map_error(&error, WinHttpOperation::QueryHeaders))
    }

    unsafe fn query_option(&self, handle: RawHandle, option: u32, buffer: Option<NonNull<u8>>, buffer_len: &mut u32) -> Result<()> {
        let buffer = buffer.map(|pointer| pointer.as_ptr().cast());

        // SAFETY: the caller guarantees the optional output buffer capacity.
        unsafe { WinHttpQueryOption(handle.as_ptr(), option, buffer, buffer_len) }
            .map_err(|error| Self::map_error(&error, WinHttpOperation::QueryOption))
    }

    fn query_data_available(&self, request: RawHandle) -> Result<()> {
        // SAFETY: the request handle is non-null; asynchronous sessions must
        // use a null OUT pointer and receive the value through the callback.
        unsafe { WinHttpQueryDataAvailable(request.as_ptr(), null_mut()) }
            .map_err(|error| Self::map_error(&error, WinHttpOperation::QueryDataAvailable))
    }

    unsafe fn read_data(&self, request: RawHandle, buffer: NonNull<u8>, len: u32) -> Result<()> {
        // SAFETY: the caller upholds the asynchronous buffer-lifetime
        // contract; the asynchronous OUT pointer is intentionally null.
        unsafe { WinHttpReadData(request.as_ptr(), buffer.as_ptr().cast(), len, null_mut()) }
            .map_err(|error| Self::map_error(&error, WinHttpOperation::ReadData))
    }

    fn close_handle(&self, handle: RawHandle) -> Result<()> {
        // SAFETY: the typed handle is non-null and ownership ensures this
        // close operation is issued at most once.
        unsafe { WinHttpCloseHandle(handle.as_ptr()) }.map_err(|error| Self::map_error(&error, WinHttpOperation::CloseHandle))
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::c_void;
    use std::ptr::null_mut;

    use super::RealBindings;
    use crate::error::{WinHttpError, WinHttpOperation};

    #[test]
    fn null_handle_path_captures_failure() {
        let error = RealBindings::handle_result_with(null_mut::<c_void>(), || WinHttpError::new(1234, WinHttpOperation::OpenRequest))
            .expect_err("a null WinHTTP handle must fail");

        assert_eq!(error.code(), 1234);
        assert_eq!(error.operation(), WinHttpOperation::OpenRequest);
    }

    #[test]
    fn callback_sentinel_path_captures_failure() {
        let error = RealBindings::callback_result_with(Some(usize::MAX), || WinHttpError::new(5678, WinHttpOperation::SetStatusCallback))
            .expect_err("the invalid callback sentinel must fail");

        assert_eq!(error.code(), 5678);
        assert_eq!(error.operation(), WinHttpOperation::SetStatusCallback);
    }

    #[test]
    fn ordinary_callback_result_succeeds() {
        RealBindings::callback_result_with(None, || WinHttpError::new(1, WinHttpOperation::SetStatusCallback))
            .expect("an ordinary callback return value succeeds");
    }
}
