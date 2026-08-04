// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::ffi::c_void;
use std::ptr::{NonNull, with_exposed_provenance_mut};

use windows::Win32::Networking::WinHttp::{
    ERROR_WINHTTP_OPERATION_CANCELLED, WINHTTP_ASYNC_RESULT, WINHTTP_CALLBACK_STATUS_CONNECTED_TO_SERVER,
    WINHTTP_CALLBACK_STATUS_CONNECTING_TO_SERVER, WINHTTP_CALLBACK_STATUS_DATA_AVAILABLE, WINHTTP_CALLBACK_STATUS_HANDLE_CLOSING,
    WINHTTP_CALLBACK_STATUS_HANDLE_CREATED, WINHTTP_CALLBACK_STATUS_HEADERS_AVAILABLE, WINHTTP_CALLBACK_STATUS_READ_COMPLETE,
    WINHTTP_CALLBACK_STATUS_REQUEST_ERROR, WINHTTP_CALLBACK_STATUS_SECURE_FAILURE, WINHTTP_CALLBACK_STATUS_SENDREQUEST_COMPLETE,
    WINHTTP_CALLBACK_STATUS_WRITE_COMPLETE,
};

use crate::context::{ActiveOperation, CompletionResult, OperationBuffer, RequestContext};
use crate::error::WinHttpError;

/// Implements the WinHTTP status-callback contract for request handles.
///
/// The function is registered through `WinHttpSetStatusCallback` and receives
/// the `WINHTTP_STATUS_CALLBACK` parameters documented by Microsoft. It does no
/// blocking work: notifications are decoded and handed to the request future or
/// recorded in request-scoped atomic state.
///
/// # Safety
///
/// `context`, `status_info`, and `status_info_len` must satisfy the
/// [`WINHTTP_STATUS_CALLBACK`] contract. A nonzero `context` must be the value
/// installed by this crate for the request handle.
///
/// [`WINHTTP_STATUS_CALLBACK`]: https://learn.microsoft.com/windows/win32/api/winhttp/nc-winhttp-winhttp_status_callback
pub(crate) unsafe extern "system" fn status_callback(
    _handle: *mut c_void,
    context: usize,
    status: u32,
    status_info: *mut c_void,
    status_info_len: u32,
) {
    let context = with_exposed_provenance_mut::<RequestContext>(context);

    // SAFETY: WinHTTP supplies either the null context value or the exact
    // pointer installed through WINHTTP_OPTION_CONTEXT_VALUE. The registration
    // contract keeps a non-null context alive through HANDLE_CLOSING.
    unsafe {
        dispatch_completion(context, status, status_info, status_info_len);
    }
}

/// Dispatches one WinHTTP status notification.
///
/// # Safety
///
/// A non-null `context` must be the exact pointer produced by
/// `plurality::Box::into_raw` for a live [`RequestContext`]. The request callback
/// contract guarantees that `HANDLE_CLOSING` is not reentrant with another
/// notification for the same request and that no later notification follows.
/// [`WinHttpCloseHandle`] also requires the context binding to remain alive
/// until that final notification.
///
/// Between installation and `HANDLE_CLOSING`, no exclusive reference to the
/// context may exist. The request task and callbacks access it only through
/// shared references; mutation is confined to its atomic fields and operation
/// slot. The final callback is the only code permitted to reconstruct the
/// owning box and therefore obtains exclusive destruction rights.
///
/// [`WinHttpCloseHandle`]: https://learn.microsoft.com/windows/win32/api/winhttp/nf-winhttp-winhttpclosehandle
pub(crate) unsafe fn dispatch_completion(context: *mut RequestContext, status: u32, status_info: *mut c_void, status_info_len: u32) {
    let Some(context) = NonNull::new(context) else {
        return;
    };

    #[expect(
        clippy::match_same_arms,
        reason = "HANDLE_CREATED is decoded explicitly while unsupported statuses are ignored"
    )]
    // Required co-edit: every status this match acts on must also appear in
    // `DISPATCHED_STATUSES` (session.rs) or, for awaited operation completions,
    // in `OperationKind::ALL` (context.rs). Those lists drive the test that
    // proves `SESSION_NOTIFICATION_FLAGS` lets `WinHTTP` deliver the
    // notification at all; a status dispatched here but absent from them is
    // untested, and if its flag is missing from the mask the affected request
    // waits forever rather than failing.
    match status {
        WINHTTP_CALLBACK_STATUS_SECURE_FAILURE => {
            if let Some(flags) = read_status_info::<u32>(status_info, status_info_len) {
                // SAFETY: the dispatch contract keeps the allocation live and
                // forbids exclusive references before HANDLE_CLOSING. This
                // shared access mutates only atomic state.
                unsafe { context.as_ref() }.record_secure_failure(flags);
            }
        }
        WINHTTP_CALLBACK_STATUS_CONNECTING_TO_SERVER => {
            // SAFETY: the dispatch contract keeps the allocation live and
            // forbids exclusive references before HANDLE_CLOSING. This shared
            // access mutates only atomic state.
            unsafe { context.as_ref() }.mark_connecting();
        }
        WINHTTP_CALLBACK_STATUS_CONNECTED_TO_SERVER => {
            // SAFETY: the dispatch contract keeps the allocation live and
            // forbids exclusive references before HANDLE_CLOSING. This shared
            // access mutates only atomic state.
            unsafe { context.as_ref() }.mark_connected();
        }
        WINHTTP_CALLBACK_STATUS_HANDLE_CREATED => {}
        WINHTTP_CALLBACK_STATUS_HANDLE_CLOSING => {
            // SAFETY: this dispatch contract guarantees that HANDLE_CLOSING is
            // the final, exactly-once notification for the handle and that it
            // does not overlap another callback for it, which is exactly what
            // close_context requires to reclaim the pooled box.
            unsafe {
                close_context(context);
            }
        }
        WINHTTP_CALLBACK_STATUS_REQUEST_ERROR => {
            // SAFETY: the dispatch contract keeps the allocation live and
            // forbids exclusive references before HANDLE_CLOSING. The
            // operation slot uses interior mutability with atomic ownership.
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

                    CompletionResult::error(error, active.buffer)
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
            // SAFETY: the dispatch contract keeps the allocation live and
            // forbids exclusive references before HANDLE_CLOSING. The
            // operation slot uses interior mutability with atomic ownership.
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

/// Fails any still-armed operation and reclaims the request context.
///
/// # Safety
///
/// `context` must be the exact pointer produced by `plurality::Box::into_raw`
/// for this context and installed as the request handle's context value. The
/// caller must be the final, exactly-once `HANDLE_CLOSING` notification for
/// that handle, and no other callback for the handle may be executing
/// concurrently: this function claims any armed operation and then drops the
/// storage holding both the operation payload and the embedded completion
/// event, so an overlapping callback would race that reclamation.
unsafe fn close_context(context: NonNull<RequestContext>) {
    // SAFETY: the dispatch contract requires HANDLE_CLOSING to run only after
    // all other callbacks finish. No exclusive reference has existed since
    // installation, so this final shared borrow cannot alias one.
    let context_ref = unsafe { context.as_ref() };

    if let Some(ActiveOperation { kind, completion, buffer }) = context_ref.take_any() {
        let mut error = WinHttpError::new(ERROR_WINHTTP_OPERATION_CANCELLED, kind.operation());
        if let Some(flags) = context_ref.secure_failure_flags() {
            error = error.with_secure_failure_flags(flags);
        }
        completion.send(CompletionResult::error(error, buffer));
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
