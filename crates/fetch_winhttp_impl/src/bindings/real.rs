// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::ffi::c_void;
use std::ptr::{NonNull, null, null_mut};

use widestring::U16CStr;
use windows::Win32::Foundation::{ERROR_IO_PENDING, ERROR_SUCCESS, GetLastError};
use windows::Win32::Networking::WinHttp::{
    WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_OPEN_REQUEST_FLAGS, WinHttpCloseHandle, WinHttpConnect, WinHttpOpen, WinHttpOpenRequest,
    WinHttpQueryHeaders, WinHttpQueryOption, WinHttpReadDataEx, WinHttpReceiveResponse, WinHttpSendRequest, WinHttpSetOption,
    WinHttpSetStatusCallback, WinHttpSetTimeouts, WinHttpWriteData,
};
use windows::core::{Error as WindowsError, PCWSTR};

use super::{Bindings, StatusCallback, WINHTTP_READ_DATA_EX_FLAG_FILL_BUFFER};
use crate::error::{Result, WinHttpError, WinHttpOperation};
use crate::handle::RawHandle;

#[derive(Clone, Copy, Debug, Default)]
/// Discharges the [`Bindings`] contract against the live WinHTTP API.
///
/// This is the only place in the crate where a WinHTTP entry point is called.
/// Every method is a one-to-one wrapper that adds nothing beyond null-handle
/// detection, Win32 error capture, and the argument conventions an asynchronous
/// session requires, such as the null output pointers that force completions
/// through the status callback (implementation.md, "The bindings facade").
/// Concentrating the calls here means the caller-side invariants documented on
/// [`Bindings`] are the complete set of obligations the transport must uphold
/// for its use of the operating system to be sound.
///
/// The type is a zero-sized unit struct, so any call site that needs production
/// behavior can materialize one without owning, borrowing, or threading state.
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

    /// Interprets a Win32 status code returned by value rather than through
    /// `GetLastError`.
    ///
    /// `ERROR_IO_PENDING` reports that the operation was accepted and will
    /// finish through the status callback, which on an asynchronous session is
    /// the ordinary outcome and not a failure.
    fn status_result(status: u32, operation: WinHttpOperation) -> Result<()> {
        if status == ERROR_SUCCESS.0 || status == ERROR_IO_PENDING.0 {
            Ok(())
        } else {
            Err(WinHttpError::new(status, operation))
        }
    }
}

// SAFETY: Every method is a direct WinHTTP call that preserves the native
// handle, callback, buffer, and completion semantics required by Bindings.
unsafe impl Bindings for RealBindings {
    unsafe fn open(&self, user_agent: &U16CStr, flags: u32) -> Result<RawHandle> {
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

    unsafe fn set_timeouts(&self, handle: RawHandle, resolve: i32, connect: i32, send: i32, receive: i32) -> Result<()> {
        // SAFETY: the typed handle is non-null and the timeout values use the
        // exact WinHttpSetTimeouts ABI types.
        unsafe { WinHttpSetTimeouts(handle.as_ptr(), resolve, connect, send, receive) }
            .map_err(|error| Self::map_error(&error, WinHttpOperation::SetTimeouts))
    }

    #[expect(
        clippy::fn_to_numeric_cast_any,
        reason = "the WinHTTP failure sentinel is the function-pointer bit pattern usize::MAX"
    )]
    unsafe fn set_status_callback(&self, handle: RawHandle, callback: StatusCallback, notification_flags: u32) -> Result<()> {
        // SAFETY: the typed handle is non-null, the callback has the required
        // system ABI, and the reserved parameter is zero as required.
        let previous = unsafe { WinHttpSetStatusCallback(handle.as_ptr(), callback, notification_flags, 0) };

        Self::callback_result(previous.map(|callback| callback as usize), WinHttpOperation::SetStatusCallback)
    }

    unsafe fn connect(&self, session: RawHandle, host: &U16CStr, port: u16) -> Result<RawHandle> {
        // SAFETY: the session is non-null and host is NUL-terminated for the
        // duration of the call; the reserved value is zero.
        let handle = unsafe { WinHttpConnect(session.as_ptr(), PCWSTR(host.as_ptr()), port, 0) };

        Self::handle_result(handle, WinHttpOperation::Connect)
    }

    unsafe fn open_request(&self, connect: RawHandle, method: &U16CStr, path: &U16CStr, flags: u32) -> Result<RawHandle> {
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

    unsafe fn set_option(&self, handle: RawHandle, option: u32, value: &[u8]) -> Result<()> {
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

    unsafe fn receive_response(&self, request: RawHandle) -> Result<()> {
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

    unsafe fn read_data_ex(&self, request: RawHandle, buffer: NonNull<u8>, len: u32, fill_buffer: bool) -> Result<()> {
        let flags = if fill_buffer { WINHTTP_READ_DATA_EX_FLAG_FILL_BUFFER } else { 0 };

        // SAFETY: the caller upholds the asynchronous buffer-lifetime
        // contract; the asynchronous OUT pointer is intentionally null. The
        // reserved property parameters are passed empty as the API requires.
        let status = unsafe { WinHttpReadDataEx(request.as_ptr(), buffer.as_ptr().cast(), len, null_mut(), flags, 0, None) };

        Self::status_result(status, WinHttpOperation::ReadData)
    }

    unsafe fn close_handle(&self, handle: RawHandle) -> Result<()> {
        // SAFETY: the typed handle is non-null and ownership ensures this
        // close operation is issued at most once.
        unsafe { WinHttpCloseHandle(handle.as_ptr()) }.map_err(|error| Self::map_error(&error, WinHttpOperation::CloseHandle))
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::ffi::c_void;
    use std::panic::{RefUnwindSafe, UnwindSafe};
    use std::ptr::null_mut;

    use static_assertions::assert_impl_all;
    use windows::Win32::Foundation::{ERROR_IO_PENDING, ERROR_SUCCESS, SetLastError, WIN32_ERROR};

    use super::RealBindings;
    use crate::bindings::StatusCallback;
    use crate::error::{WinHttpError, WinHttpOperation};

    assert_impl_all!(StatusCallback: UnwindSafe, RefUnwindSafe);
    assert_impl_all!(RealBindings: UnwindSafe, RefUnwindSafe);

    #[test]
    fn null_handle_path_captures_failure() {
        let error =
            RealBindings::handle_result_with(null_mut::<c_void>(), || WinHttpError::new(1234, WinHttpOperation::OpenRequest)).unwrap_err();

        assert_eq!(error.code(), 1234);
        assert_eq!(error.operation(), WinHttpOperation::OpenRequest);
    }

    #[test]
    fn callback_sentinel_path_captures_failure() {
        let error = RealBindings::callback_result_with(Some(usize::MAX), || WinHttpError::new(5678, WinHttpOperation::SetStatusCallback))
            .unwrap_err();

        assert_eq!(error.code(), 5678);
        assert_eq!(error.operation(), WinHttpOperation::SetStatusCallback);
    }

    #[test]
    fn a_status_returned_by_value_accepts_success_and_pending_and_preserves_any_other_code() {
        // WinHttpReadDataEx reports its outcome in the return value instead of
        // the last-error slot, and on an asynchronous session the ordinary
        // outcome is a pending read that the status callback will finish.
        RealBindings::status_result(ERROR_SUCCESS.0, WinHttpOperation::ReadData).unwrap();
        RealBindings::status_result(ERROR_IO_PENDING.0, WinHttpOperation::ReadData).unwrap();

        let error = RealBindings::status_result(1234, WinHttpOperation::ReadData).unwrap_err();

        assert_eq!(error.code(), 1234);
        assert_eq!(error.operation(), WinHttpOperation::ReadData);
    }

    #[test]
    fn a_failed_native_call_reports_the_operating_system_failure_code() {
        // WinHTTP reports failure by returning a null handle and leaving the
        // reason in the thread's last-error slot, so that slot is the only
        // input the production path has. `ERROR_WINHTTP_INTERNAL_ERROR` stands
        // in for any code the operating system might leave there.
        const ERROR_WINHTTP_INTERNAL_ERROR: u32 = 12004;

        // SAFETY: SetLastError has no preconditions and only writes the calling
        // thread's own last-error slot, which this thread reads back below.
        unsafe { SetLastError(WIN32_ERROR(ERROR_WINHTTP_INTERNAL_ERROR)) };

        let error = RealBindings::handle_result(null_mut::<c_void>(), WinHttpOperation::Connect).unwrap_err();

        assert_eq!(error.code(), ERROR_WINHTTP_INTERNAL_ERROR);
        assert_eq!(error.operation(), WinHttpOperation::Connect);
    }

    #[test]
    fn ordinary_callback_result_succeeds() {
        RealBindings::callback_result_with(None, || WinHttpError::new(1, WinHttpOperation::SetStatusCallback)).unwrap();
    }
}
