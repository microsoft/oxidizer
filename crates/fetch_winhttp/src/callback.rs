// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::ffi::c_void;
use std::ptr::NonNull;

use windows::Win32::Networking::WinHttp::{
    WINHTTP_ASYNC_RESULT, WINHTTP_CALLBACK_STATUS_CONNECTED_TO_SERVER, WINHTTP_CALLBACK_STATUS_CONNECTING_TO_SERVER,
    WINHTTP_CALLBACK_STATUS_DATA_AVAILABLE, WINHTTP_CALLBACK_STATUS_HANDLE_CLOSING, WINHTTP_CALLBACK_STATUS_HANDLE_CREATED,
    WINHTTP_CALLBACK_STATUS_HEADERS_AVAILABLE, WINHTTP_CALLBACK_STATUS_READ_COMPLETE, WINHTTP_CALLBACK_STATUS_REQUEST_ERROR,
    WINHTTP_CALLBACK_STATUS_SECURE_FAILURE, WINHTTP_CALLBACK_STATUS_SENDREQUEST_COMPLETE, WINHTTP_CALLBACK_STATUS_WRITE_COMPLETE,
};

use crate::context::{ActiveOperation, CompletionResult, OperationBuffer, RequestContext};
use crate::error::WinHttpError;

const ERROR_WINHTTP_OPERATION_CANCELLED: u32 = 12017;

pub(crate) unsafe extern "system" fn status_callback(
    _handle: *mut c_void,
    context: usize,
    status: u32,
    status_info: *mut c_void,
    status_info_len: u32,
) {
    let context = std::ptr::with_exposed_provenance_mut::<RequestContext>(context);

    // SAFETY: WinHTTP supplies either the null context value or the exact
    // pointer installed through WINHTTP_OPTION_CONTEXT_VALUE. The registration
    // contract keeps a non-null context alive through HANDLE_CLOSING.
    unsafe {
        dispatch_completion(context, status, status_info, status_info_len);
    }
}

/// Dispatches one `WinHTTP` status notification.
///
/// # Safety
///
/// A non-null `context` must be the exact pointer produced by
/// `plurality::Box::into_raw` for a live [`RequestContext`]. The request callback
/// protocol must deliver `HANDLE_CLOSING` exactly once and as its final use.
pub(crate) unsafe fn dispatch_completion(context: *mut RequestContext, status: u32, status_info: *mut c_void, status_info_len: u32) {
    let Some(context) = NonNull::new(context) else {
        return;
    };

    #[expect(
        clippy::match_same_arms,
        reason = "HANDLE_CREATED is decoded explicitly while unsupported statuses are ignored"
    )]
    match status {
        WINHTTP_CALLBACK_STATUS_SECURE_FAILURE => {
            if let Some(flags) = read_status_info::<u32>(status_info, status_info_len) {
                // SAFETY: the dispatch contract guarantees the context remains
                // alive. This method touches only an atomic diagnostic field.
                unsafe { context.as_ref() }.record_secure_failure(flags);
            }
        }
        WINHTTP_CALLBACK_STATUS_CONNECTING_TO_SERVER => {
            // SAFETY: the dispatch contract guarantees the context remains
            // alive. This method touches only an atomic attribution field.
            unsafe { context.as_ref() }.mark_connecting();
        }
        WINHTTP_CALLBACK_STATUS_CONNECTED_TO_SERVER => {
            // SAFETY: the dispatch contract guarantees the context remains
            // alive. This method touches only an atomic attribution field.
            unsafe { context.as_ref() }.mark_connected();
        }
        WINHTTP_CALLBACK_STATUS_HANDLE_CREATED => {}
        WINHTTP_CALLBACK_STATUS_HANDLE_CLOSING => {
            // SAFETY: HANDLE_CLOSING is the final callback and therefore has
            // exclusive reclamation authority over the pooled box.
            unsafe {
                close_context(context);
            }
        }
        WINHTTP_CALLBACK_STATUS_REQUEST_ERROR => {
            // SAFETY: the dispatch contract guarantees the context is valid
            // for this non-final callback.
            let context_ref = unsafe { context.as_ref() };
            let Some(active) = context_ref.take_any() else {
                return;
            };

            let result = match read_status_info::<WINHTTP_ASYNC_RESULT>(status_info, status_info_len) {
                Some(async_result) => {
                    let mut error = WinHttpError::new(async_result.dwError, active.kind.operation());
                    if let Some(flags) = context_ref.secure_failure_flags() {
                        error = error.with_secure_failure_flags(flags);
                    }

                    CompletionResult::error(error, Some(async_result.dwResult), active.buffer)
                }
                None => CompletionResult::invalid_status_info(status, status_info_len, active.buffer),
            };

            active.completion.send(result);
        }
        WINHTTP_CALLBACK_STATUS_SENDREQUEST_COMPLETE
        | WINHTTP_CALLBACK_STATUS_HEADERS_AVAILABLE
        | WINHTTP_CALLBACK_STATUS_DATA_AVAILABLE
        | WINHTTP_CALLBACK_STATUS_READ_COMPLETE
        | WINHTTP_CALLBACK_STATUS_WRITE_COMPLETE => {
            // SAFETY: the dispatch contract guarantees the context is valid
            // for this non-final callback.
            let Some(active) = (unsafe { context.as_ref() }).take_for_status(status) else {
                return;
            };

            let result = decode_success(status, status_info, status_info_len, active.buffer);
            active.completion.send(result);
        }
        _ => {}
    }
}

fn decode_success(status: u32, status_info: *mut c_void, status_info_len: u32, buffer: OperationBuffer) -> CompletionResult {
    match status {
        WINHTTP_CALLBACK_STATUS_SENDREQUEST_COMPLETE => match buffer {
            OperationBuffer::None => CompletionResult::SendRequestComplete,
            unexpected => CompletionResult::invalid_status_info(status, status_info_len, unexpected),
        },
        WINHTTP_CALLBACK_STATUS_HEADERS_AVAILABLE => match buffer {
            OperationBuffer::None => CompletionResult::HeadersAvailable,
            unexpected => CompletionResult::invalid_status_info(status, status_info_len, unexpected),
        },
        WINHTTP_CALLBACK_STATUS_DATA_AVAILABLE => match (read_status_info::<u32>(status_info, status_info_len), buffer) {
            (Some(available), OperationBuffer::None) => CompletionResult::DataAvailable(available),
            (_, unexpected) => CompletionResult::invalid_status_info(status, status_info_len, unexpected),
        },
        WINHTTP_CALLBACK_STATUS_READ_COMPLETE => decode_read(status, status_info, status_info_len, buffer),
        WINHTTP_CALLBACK_STATUS_WRITE_COMPLETE => match (read_status_info::<u32>(status_info, status_info_len), buffer) {
            (Some(written), OperationBuffer::Write { buffer, len, .. }) if written <= len => {
                CompletionResult::WriteComplete { buffer, len: written }
            }
            (_, unexpected) => CompletionResult::invalid_status_info(status, status_info_len, unexpected),
        },
        _ => CompletionResult::invalid_status_info(status, status_info_len, buffer),
    }
}

fn decode_read(status: u32, status_info: *mut c_void, status_info_len: u32, buffer: OperationBuffer) -> CompletionResult {
    let OperationBuffer::Read { buffer, address, capacity } = buffer else {
        return CompletionResult::invalid_status_info(status, status_info_len, buffer);
    };

    let returned_address = status_info.addr();
    let address_matches = returned_address == address || (status_info_len == 0 && status_info.is_null());

    if status_info_len <= capacity && address_matches {
        CompletionResult::ReadComplete {
            buffer,
            len: status_info_len,
        }
    } else {
        CompletionResult::invalid_status_info(status, status_info_len, OperationBuffer::Read { buffer, address, capacity })
    }
}

fn read_status_info<T: Copy>(status_info: *mut c_void, status_info_len: u32) -> Option<T> {
    let expected_len = u32::try_from(size_of::<T>()).ok()?;

    if status_info.is_null() || status_info_len != expected_len || !status_info.addr().is_multiple_of(align_of::<T>()) {
        return None;
    }

    // SAFETY: nullness, exact byte length, and alignment were validated above.
    // WinHTTP owns the source for the callback duration, and Copy prevents any
    // destructor or ownership from being duplicated by this value read.
    Some(unsafe { status_info.cast::<T>().read() })
}

unsafe fn close_context(context: NonNull<RequestContext>) {
    // SAFETY: HANDLE_CLOSING is documented as the final callback, so no other
    // callback can access the context after this point.
    let context_ref = unsafe { context.as_ref() };

    if let Some(ActiveOperation { kind, completion, buffer }) = context_ref.take_any() {
        let mut error = WinHttpError::new(ERROR_WINHTTP_OPERATION_CANCELLED, kind.operation());
        if let Some(flags) = context_ref.secure_failure_flags() {
            error = error.with_secure_failure_flags(flags);
        }
        completion.send(CompletionResult::error(error, None, buffer));
    }

    // SAFETY: this is the exact pointer produced by plurality::Box::into_raw.
    // The callback protocol grants HANDLE_CLOSING sole, exactly-once ownership
    // to reconstruct and drop it.
    drop(unsafe { plurality::Box::<RequestContext>::from_raw(context) });
}

#[cfg(test)]
mod tests {
    use std::ffi::c_void;
    use std::ptr::null_mut;

    use windows::Win32::Networking::WinHttp::{
        WINHTTP_CALLBACK_STATUS_HANDLE_CLOSING, WINHTTP_CALLBACK_STATUS_REQUEST_ERROR, WINHTTP_CALLBACK_STATUS_SECURE_FAILURE,
    };

    use super::{dispatch_completion, status_callback};

    #[test]
    fn null_context_callbacks_do_nothing() {
        // SAFETY: a null context is explicitly accepted and ignored.
        unsafe {
            dispatch_completion(null_mut(), u32::MAX, null_mut(), 0);
        }
        for status in [
            WINHTTP_CALLBACK_STATUS_SECURE_FAILURE,
            WINHTTP_CALLBACK_STATUS_REQUEST_ERROR,
            WINHTTP_CALLBACK_STATUS_HANDLE_CLOSING,
        ] {
            // SAFETY: a zero context is explicitly accepted and ignored before
            // the deliberately invalid status-info pointer is inspected.
            unsafe {
                status_callback(null_mut(), 0, status, std::ptr::dangling_mut::<c_void>(), u32::MAX);
            }
        }
    }
}
