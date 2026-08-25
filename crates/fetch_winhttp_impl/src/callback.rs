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

use crate::context::{ActiveOperation, CompletionResult, OperationBuffer, OperationKind, RequestContext};
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
/// The caller must deliver notifications under the
/// [`WINHTTP_STATUS_CALLBACK`] contract, either as `WinHTTP` itself or as a
/// test double that models it exactly. Specifically:
///
/// - `context` must be zero, or the address this crate installed for the
///   notifying request handle through `WINHTTP_OPTION_CONTEXT_VALUE`, whose
///   allocation must not yet have been reclaimed.
/// - `status_info` must be null or readable for `status_info_len` initialized
///   bytes, and must stay valid and unmodified until this function returns. A
///   zero `context` places no requirement on the payload, which is then never
///   examined.
/// - `WINHTTP_CALLBACK_STATUS_HANDLE_CLOSING` must be the last notification
///   delivered for a handle, must be delivered exactly once for a handle whose
///   context was installed, and must not overlap another notification for that
///   same handle.
///
/// [`WINHTTP_STATUS_CALLBACK`]: https://learn.microsoft.com/windows/win32/api/winhttp/nc-winhttp-winhttp_status_callback
pub(crate) unsafe extern "system" fn status_callback(
    _handle: *mut c_void,
    context: usize,
    status: u32,
    status_info: *mut c_void,
    status_info_len: u32,
) {
    // The context survives the round trip through WinHTTP only as an address,
    // so the pointer is rebuilt from exposed provenance. `RequestGuard`
    // publishes that provenance with `expose_provenance` before the address is
    // handed to WinHTTP, which is what makes this reconstruction address the
    // context allocation rather than carry no provenance at all.
    let context = with_exposed_provenance_mut::<RequestContext>(context);

    // SAFETY: dispatch_completion requires a null context or the exact
    // plurality::Box::into_raw pointer of a live context, a payload readable
    // for status_info_len bytes, a HANDLE_CLOSING notification that is final,
    // exactly-once and non-overlapping, and the absence of exclusive references
    // to an installed context. The payload and notification-ordering
    // requirements are demanded of this function's caller unchanged. The
    // context requirement is met by the reconstruction above: this function's
    // caller supplies the address ContextInstallation::install published, and
    // that address carries the provenance of the exact into_raw pointer it was
    // taken from. No exclusive reference exists by construction: after
    // installation this crate forms only shared references to a context, and
    // the sole exclusive one is created by close_context below, which the
    // contract's non-overlap and finality rules keep disjoint from every other
    // notification.
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
/// notification for the same request, that it arrives exactly once for an
/// installed context, and that no later notification follows.
/// [`WinHttpCloseHandle`] also requires the context binding to remain alive
/// until that final notification.
///
/// `status_info` must be null, or readable and initialized for
/// `status_info_len` bytes that stay valid and unmodified until this function
/// returns; the payload is only ever read, never retained. A null `context`
/// places no requirement on the payload, which is then never examined, and
/// neither does `WINHTTP_CALLBACK_STATUS_READ_COMPLETE`, whose payload states
/// the address and byte count of the buffer the read filled and is compared
/// against the submitted buffer rather than dereferenced.
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
    // proves `session_notification_flags` lets `WinHTTP` deliver the
    // notification at all; a status dispatched here but absent from them is
    // untested, and if its flag is missing from the mask the affected request
    // waits forever rather than failing.
    match status {
        WINHTTP_CALLBACK_STATUS_SECURE_FAILURE => {
            // SAFETY: read_status_info requires a null or readable payload that
            // stays unmodified for the call, which this function's contract
            // demands of its caller verbatim.
            if let Some(flags) = unsafe { read_status_info::<u32>(status_info, status_info_len) } {
                // SAFETY: NonNull::as_ref requires an aligned, dereferenceable
                // pointer to an initialized value that stays live and free of
                // exclusive references for the borrow. The contract admits only
                // the into_raw pointer of a live pooled context, reclamation
                // happens solely in the non-overlapping final notification, and
                // no exclusive reference to an installed context exists. The
                // borrow ends with this statement, which mutates only atomics.
                unsafe { context.as_ref() }.record_secure_failure(flags);
            }
        }
        WINHTTP_CALLBACK_STATUS_CONNECTING_TO_SERVER => {
            // SAFETY: NonNull::as_ref requires an aligned, dereferenceable
            // pointer to an initialized value that stays live and free of
            // exclusive references for the borrow. The contract admits only the
            // into_raw pointer of a live pooled context, reclamation happens
            // solely in the non-overlapping final notification, and no
            // exclusive reference to an installed context exists. The borrow
            // ends with this statement, which mutates only atomics.
            unsafe { context.as_ref() }.mark_connecting();
        }
        WINHTTP_CALLBACK_STATUS_CONNECTED_TO_SERVER => {
            // SAFETY: NonNull::as_ref requires an aligned, dereferenceable
            // pointer to an initialized value that stays live and free of
            // exclusive references for the borrow. The contract admits only the
            // into_raw pointer of a live pooled context, reclamation happens
            // solely in the non-overlapping final notification, and no
            // exclusive reference to an installed context exists. The borrow
            // ends with this statement, which mutates only atomics.
            unsafe { context.as_ref() }.mark_connected();
        }
        WINHTTP_CALLBACK_STATUS_HANDLE_CREATED => {}
        WINHTTP_CALLBACK_STATUS_HANDLE_CLOSING => {
            // SAFETY: close_context requires the exact into_raw pointer of the
            // installed context, sole reclamation authority, and no concurrent
            // callback for the handle. This function's contract supplies the
            // pointer, and it makes HANDLE_CLOSING the final, exactly-once,
            // non-overlapping notification for the handle, so this arm both
            // holds that authority alone and runs disjointly from every other
            // callback that touches the context.
            unsafe {
                close_context(context);
            }
        }
        WINHTTP_CALLBACK_STATUS_REQUEST_ERROR => {
            // SAFETY: NonNull::as_ref requires an aligned, dereferenceable
            // pointer to an initialized value that stays live and free of
            // exclusive references for the borrow. The contract admits only the
            // into_raw pointer of a live pooled context, reclamation happens
            // solely in the non-overlapping final notification, and no
            // exclusive reference to an installed context exists. The borrow
            // ends within this arm, and it reaches the operation payload only
            // through the slot's atomic ownership transfer.
            let context_ref = unsafe { context.as_ref() };
            let Some(active) = context_ref.take_any() else {
                return;
            };

            // SAFETY: read_status_info requires a null or readable payload that
            // stays unmodified for the call, which this function's contract
            // demands of its caller verbatim.
            let result = match unsafe { read_status_info::<WINHTTP_ASYNC_RESULT>(status_info, status_info_len) } {
                Some(async_result) => {
                    CompletionResult::error(diagnosed_error(context_ref, async_result.dwError, active.kind), active.buffer)
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
            // SAFETY: NonNull::as_ref requires an aligned, dereferenceable
            // pointer to an initialized value that stays live and free of
            // exclusive references for the borrow. The contract admits only the
            // into_raw pointer of a live pooled context, reclamation happens
            // solely in the non-overlapping final notification, and no
            // exclusive reference to an installed context exists. The borrow
            // ends with this statement, and it reaches the operation payload
            // only through the slot's atomic ownership transfer.
            let Some(active) = (unsafe { context.as_ref() }).take_for_status(status) else {
                return;
            };

            // SAFETY: decode_success requires a null or readable payload that
            // stays unmodified for the call, which this function's contract
            // demands of its caller verbatim.
            let result = unsafe { decode_success(status, status_info, status_info_len, active.buffer) };
            active.completion.send(result);
        }
        _ => {}
    }
}

/// Attributes a `WinHTTP` failure to an operation and its TLS diagnostics.
///
/// A secure failure notification carries the reason a TLS handshake was
/// rejected but does not itself end the request, and `WinHTTP` documents no
/// order between it and the failure that follows. Recording the flags on the
/// context and reading them back here attaches them to whichever notification
/// ends the operation, whether that is a request error or a cancellation.
fn diagnosed_error(context: &RequestContext, code: u32, kind: OperationKind) -> WinHttpError {
    let error = WinHttpError::new(code, kind.operation());

    match context.secure_failure_flags() {
        Some(flags) => error.with_secure_failure_flags(flags),
        None => error,
    }
}

/// Decodes a successful completion notification into its result.
///
/// # Safety
///
/// `status_info` must be null, or readable and initialized for
/// `status_info_len` bytes that stay valid and unmodified until this function
/// returns. `WINHTTP_CALLBACK_STATUS_READ_COMPLETE` is exempt: its payload
/// states the address and byte count of the buffer the read filled, which
/// [`decode_read`] compares against the submitted buffer instead of reading
/// through it.
unsafe fn decode_success(status: u32, status_info: *mut c_void, status_info_len: u32, buffer: OperationBuffer) -> CompletionResult {
    match status {
        WINHTTP_CALLBACK_STATUS_SENDREQUEST_COMPLETE => match buffer {
            OperationBuffer::None => CompletionResult::SendRequestComplete,
            unexpected => CompletionResult::invalid_status_info(status, status_info_len, unexpected),
        },
        WINHTTP_CALLBACK_STATUS_HEADERS_AVAILABLE => match buffer {
            OperationBuffer::None => CompletionResult::HeadersAvailable,
            unexpected => CompletionResult::invalid_status_info(status, status_info_len, unexpected),
        },
        // SAFETY: read_status_info requires a null or readable payload that
        // stays unmodified for the call, which this function's contract demands
        // of its caller verbatim.
        WINHTTP_CALLBACK_STATUS_DATA_AVAILABLE => match (unsafe { read_status_info::<u32>(status_info, status_info_len) }, buffer) {
            (Some(available), OperationBuffer::None) => CompletionResult::DataAvailable(available),
            (_, unexpected) => CompletionResult::invalid_status_info(status, status_info_len, unexpected),
        },
        WINHTTP_CALLBACK_STATUS_READ_COMPLETE => decode_read(status, status_info, status_info_len, buffer),
        // SAFETY: read_status_info requires a null or readable payload that
        // stays unmodified for the call, which this function's contract demands
        // of its caller verbatim.
        WINHTTP_CALLBACK_STATUS_WRITE_COMPLETE => match (unsafe { read_status_info::<u32>(status_info, status_info_len) }, buffer) {
            (Some(written), OperationBuffer::Write { buffer, len, .. }) if written <= len => {
                CompletionResult::WriteComplete { buffer, len: written }
            }
            (_, unexpected) => CompletionResult::invalid_status_info(status, status_info_len, unexpected),
        },
        _ => CompletionResult::invalid_status_info(status, status_info_len, buffer),
    }
}

/// Decodes a read completion, accepting it only if it describes the buffer the
/// read was submitted with.
///
/// `WinHTTP` reports the address and byte count of the filled buffer rather
/// than a payload to dereference, so this function compares the reported
/// address against the submitted one and bounds the reported count by the lent
/// capacity. Nothing is read through `status_info`, which is why the callback's
/// payload requirement does not extend to this status. A completion naming a
/// foreign address or an impossible length is rejected instead of being trusted
/// to describe initialized bytes.
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

/// Reads the notification payload as a `T`, or reports that it is not one.
///
/// The read is refused unless the payload is present, exactly as long as `T`,
/// and aligned for it, so a notification carrying a different payload shape
/// than the operation expects is reported rather than misinterpreted.
///
/// # Safety
///
/// `status_info` must be null or readable for `status_info_len` initialized
/// bytes, and must stay valid and unmodified until this function returns.
unsafe fn read_status_info<T: Copy>(status_info: *mut c_void, status_info_len: u32) -> Option<T> {
    let expected_len = u32::try_from(size_of::<T>()).ok()?;

    if status_info.is_null() || status_info_len != expected_len || !status_info.addr().is_multiple_of(align_of::<T>()) {
        return None;
    }

    // SAFETY: ptr::read requires a non-null, aligned pointer to an initialized
    // T that is readable for size_of::<T>() bytes and unmodified for the read.
    // Nullness and alignment were rejected above, and the length check proves
    // the caller's readable, initialized, unmodified payload is exactly the
    // size of T. T: Copy means the resulting value duplicates no ownership, so
    // the payload retained by the caller runs no destructor of its own.
    Some(unsafe { status_info.cast::<T>().read() })
}

/// Fails any still-armed operation and reclaims the request context.
///
/// Dropping the reclaimed context closes the retained connect handle, and the
/// session handle too when this was the last request of a released transport,
/// on whichever thread `WinHTTP` delivered the notification. That is compatible
/// with the non-blocking-callback assurance the session gives (session.rs,
/// `WINHTTP_OPTION_ASSURED_NON_BLOCKING_CALLBACKS`), because
/// [`WinHttpCloseHandle`] only starts the teardown rather than waiting for it,
/// which is why callbacks for a closed handle may still arrive after it
/// returns. The concurrency rule it must respect is that a handle may be closed
/// only while no other thread is calling `WinHTTP` with it and awaiting the
/// return. Both parents satisfy that: the connect handle belongs to this
/// request alone and is used only to open it, before the context is installed,
/// and the session handle is closed only when the last owner drops, while every
/// caller of a session-scoped function holds an owner of its own.
/// Ref: <https://learn.microsoft.com/windows/win32/winhttp/concurrency-in-winhttp>
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
///
/// [`WinHttpCloseHandle`]: https://learn.microsoft.com/windows/win32/api/winhttp/nf-winhttp-winhttpclosehandle
unsafe fn close_context(context: NonNull<RequestContext>) {
    // SAFETY: NonNull::as_ref requires an aligned, dereferenceable pointer to
    // an initialized value that stays live and free of exclusive references for
    // the borrow. This function's contract admits only the into_raw pointer of
    // the installed context, which is live until the reclamation below, and
    // makes this the sole callback for the handle, so no other callback can be
    // borrowing it. No exclusive reference has existed since installation, and
    // this borrow ends before the box is reconstructed.
    let context_ref = unsafe { context.as_ref() };

    if let Some(ActiveOperation { kind, completion, buffer }) = context_ref.take_any() {
        let error = diagnosed_error(context_ref, ERROR_WINHTTP_OPERATION_CANCELLED, kind);
        completion.send(CompletionResult::error(error, buffer));
    }

    // SAFETY: plurality::Box::from_raw requires the exact pointer returned by
    // into_raw, the same allocator type, and exactly one such reconstruction.
    // The contract supplies the pointer, and the pool's Box is the only owner
    // type this crate constructs for a context. Installation transferred the
    // sole right to reclaim it to the final notification, which the contract
    // says arrives exactly once, so no other code path reconstructs it. The
    // shared borrow above is dead, and the contract's non-overlap rule means no
    // other callback holds one.
    drop(unsafe { plurality::Box::<RequestContext>::from_raw(context) });
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::ffi::c_void;
    use std::ptr::{NonNull, null_mut};

    use bytesbuf::BytesView;
    use bytesbuf::mem::GlobalPool;
    use windows::Win32::Networking::WinHttp::{
        WINHTTP_ASYNC_RESULT, WINHTTP_CALLBACK_STATUS_HANDLE_CLOSING, WINHTTP_CALLBACK_STATUS_HEADERS_AVAILABLE,
        WINHTTP_CALLBACK_STATUS_READ_COMPLETE, WINHTTP_CALLBACK_STATUS_REQUEST_ERROR, WINHTTP_CALLBACK_STATUS_RESOLVING_NAME,
        WINHTTP_CALLBACK_STATUS_SECURE_FAILURE, WINHTTP_CALLBACK_STATUS_SENDREQUEST_COMPLETE, WINHTTP_CALLBACK_STATUS_WRITE_COMPLETE,
    };

    use super::{decode_success, dispatch_completion, status_callback};
    use crate::context::{CompletionResult, OperationBuffer, OperationKind, RequestContext};
    use crate::mocks::{complete, drive, finish, installed, status_info_len};

    #[test]
    fn null_context_callbacks_do_nothing() {
        // SAFETY: a null context is admitted by the contract, and it places no
        // requirement on the payload, which is null here regardless.
        unsafe {
            dispatch_completion(null_mut(), u32::MAX, null_mut(), 0);
        }
        for status in [
            WINHTTP_CALLBACK_STATUS_SECURE_FAILURE,
            WINHTTP_CALLBACK_STATUS_REQUEST_ERROR,
            WINHTTP_CALLBACK_STATUS_HANDLE_CLOSING,
        ] {
            // SAFETY: a zero context is admitted by the contract and places no
            // requirement on the payload, which is the point of this call: the
            // payload here is unreadable, so dispatching it would be caught.
            unsafe {
                status_callback(null_mut(), 0, status, std::ptr::dangling_mut::<c_void>(), u32::MAX);
            }
        }
    }

    /// The harness's dispatch helpers take a raw context pointer, so they
    /// reject a pointer no installation produced rather than pass it to
    /// `dispatch_completion`, whose contract admits only an installed,
    /// not-yet-reclaimed context. The check turns the most likely misuse of
    /// the helpers into a panic instead of undefined behaviour.
    #[test]
    #[should_panic(expected = "the harness delivers notifications only to a context it installed")]
    fn dispatching_to_an_uninstalled_context_is_rejected() {
        // SAFETY: complete requires an installed, not-yet-reclaimed context,
        // which this call deliberately violates to exercise the harness's own
        // rejection of it; the rejection happens before the pointer is read, so
        // no dereference occurs. Its remaining obligations hold: a send
        // completion carries no payload, this thread delivers the only
        // notification in the test, and no borrow of any context is
        // outstanding.
        unsafe {
            complete(
                std::ptr::dangling_mut::<RequestContext>(),
                WINHTTP_CALLBACK_STATUS_SENDREQUEST_COMPLETE,
                null_mut(),
                0,
            );
        }
    }

    #[test]
    fn an_unhandled_status_and_an_unarmed_error_leave_the_context_untouched() {
        let (guard, context, contexts, session, closes) = installed();

        // A status outside the dispatched set reaches the callback whenever
        // `WinHTTP` widens what a notification flag covers, and must not be
        // mistaken for a completion. A request error without an armed operation
        // arrives when the request fails after its operation was already
        // claimed. Neither may reclaim the context, which `finish` proves.
        for status in [WINHTTP_CALLBACK_STATUS_RESOLVING_NAME, WINHTTP_CALLBACK_STATUS_REQUEST_ERROR] {
            // SAFETY: complete requires an installed, not-yet-reclaimed
            // context, a payload matching the notification, no overlapping
            // notification, no outstanding exclusive borrow, and no use of the
            // context after the reclaiming notification. `installed` returned
            // the recorded pointer and only `finish` below reclaims it; a null
            // payload of zero length is readable for every status; this test
            // delivers each notification from its own thread in turn; and the
            // guard borrows the context only sharedly.
            unsafe {
                complete(context, status, null_mut(), 0);
            }
        }

        // SAFETY: finish requires an installed, not-yet-reclaimed context and
        // no overlapping notification. The deliveries above left the context
        // installed and all of them have returned; consuming the guard
        // discharges the obligation forbidding later use of it.
        unsafe {
            finish(guard, context, &contexts, session, &closes);
        }
    }

    #[test]
    fn a_completion_whose_payload_contradicts_its_operation_is_rejected() {
        // The operation slot pairs a notification with the buffer its
        // submission lent. A completion that does not fit that pairing - a
        // payload-free status arriving with a buffer, a write reporting more
        // bytes than were lent, a read naming a buffer no read was submitted
        // with, or a status this decoder never issues - describes no
        // transferable result and is reported as a protocol violation.
        let pool = GlobalPool::new();
        let write = || OperationBuffer::write(BytesView::copied_from_slice(b"ab", &pool), 2);
        let read = || OperationBuffer::read(pool.reserve(8), NonNull::<u8>::dangling().as_ptr().addr(), 8);
        let mut written = 8_u32;
        let payload = (&raw mut written).cast::<c_void>();
        let len = status_info_len::<u32>();

        for (status, buffer) in [
            (WINHTTP_CALLBACK_STATUS_SENDREQUEST_COMPLETE, write()),
            (WINHTTP_CALLBACK_STATUS_HEADERS_AVAILABLE, write()),
            (WINHTTP_CALLBACK_STATUS_WRITE_COMPLETE, write()),
            (WINHTTP_CALLBACK_STATUS_READ_COMPLETE, write()),
            (WINHTTP_CALLBACK_STATUS_RESOLVING_NAME, read()),
        ] {
            // SAFETY: decode_success requires a null or readable payload that
            // stays unmodified for the call. The payload is the initialized
            // local `written`, which outlives every iteration and which nothing
            // else can reach.
            let result = unsafe { decode_success(status, payload, len, buffer) };

            assert!(
                matches!(result, CompletionResult::InvalidStatusInfo { status: reported, .. } if reported == status),
                "status 0x{status:08x} must be reported as invalid"
            );
        }
    }

    #[test]
    fn a_read_completion_naming_a_foreign_buffer_is_rejected() {
        // A read completion is accepted only if it names the buffer the read
        // lent. The sole exemption is the empty completion reporting end of
        // stream, which arrives as a null address with a zero length. Neither
        // half of that exemption stands alone: a zero length against a foreign
        // address, or a null address against a non-zero length, names no buffer
        // this read lent and is rejected rather than trusted to describe
        // initialized bytes.
        let pool = GlobalPool::new();
        let address = NonNull::<u8>::dangling().as_ptr().addr();
        let capacity = 8_u32;
        let foreign = std::ptr::without_provenance_mut::<c_void>(address.wrapping_add(0x100));
        let submitted = std::ptr::without_provenance_mut::<c_void>(address);

        for (payload, len, accepted) in [
            (foreign, 0_u32, false),
            (null_mut(), 4, false),
            (foreign, 4, false),
            (null_mut(), 0, true),
            (submitted, 4, true),
        ] {
            let buffer = OperationBuffer::read(pool.reserve(capacity as usize), address, capacity);

            // SAFETY: decode_success requires a null or readable payload that
            // stays unmodified for the call. A read completion is exempt from
            // the readability half of that requirement, because decoding one
            // only compares the reported address and length and never reads
            // through the pointer, so the addresses named here are never
            // dereferenced.
            let result = unsafe { decode_success(WINHTTP_CALLBACK_STATUS_READ_COMPLETE, payload, len, buffer) };

            if accepted {
                assert!(
                    matches!(result, CompletionResult::ReadComplete { len: reported, .. } if reported == len),
                    "a read completion naming the lent buffer must be accepted"
                );
            } else {
                assert!(
                    matches!(result, CompletionResult::InvalidStatusInfo { .. }),
                    "a read completion of length {len} naming a foreign address must be rejected"
                );
            }
        }
    }

    #[test]
    fn duplicate_and_late_completions_cannot_send_twice() {
        let (mut guard, context, contexts, session, closes) = installed();
        let future = guard.submit(OperationKind::SendRequest, OperationBuffer::none(), |_, _| Ok(()));

        // SAFETY: complete requires an installed, not-yet-reclaimed context, a
        // payload matching the notification, no overlapping notification, no
        // outstanding exclusive borrow, and no use of the context after the
        // reclaiming notification. `installed` returned the recorded pointer
        // and only `finish` below reclaims it; a send completion carries no
        // payload; this test delivers every notification from its own thread,
        // one after another; and the guard borrows the context only sharedly.
        unsafe {
            complete(context, WINHTTP_CALLBACK_STATUS_SENDREQUEST_COMPLETE, std::ptr::null_mut(), 0);
        }
        // SAFETY: as for the delivery above; the context is still installed
        // because nothing between the two calls reclaims it.
        unsafe {
            complete(context, WINHTTP_CALLBACK_STATUS_SENDREQUEST_COMPLETE, std::ptr::null_mut(), 0);
        }
        assert!(matches!(drive(future).unwrap(), CompletionResult::SendRequestComplete));
        // SAFETY: as for the deliveries above. Awaiting the future consumes the
        // completion but leaves the context installed, so this late
        // notification still meets the contract.
        unsafe {
            complete(context, WINHTTP_CALLBACK_STATUS_SENDREQUEST_COMPLETE, std::ptr::null_mut(), 0);
        }

        // SAFETY: finish requires an installed, not-yet-reclaimed context and
        // no overlapping notification. The deliveries above left the context
        // installed and all of them have returned; consuming the guard
        // discharges the obligation forbidding later use of it.
        unsafe {
            finish(guard, context, &contexts, session, &closes);
        }
    }

    #[test]
    fn malformed_status_info_is_not_dereferenced() {
        let (mut guard, context, contexts, session, closes) = installed();
        let future = guard.submit(OperationKind::SendRequest, OperationBuffer::none(), |_, _| Ok(()));
        let mut bytes = [0_u8; size_of::<WINHTTP_ASYNC_RESULT>() + align_of::<WINHTTP_ASYNC_RESULT>()];
        let offset = (0..align_of::<WINHTTP_ASYNC_RESULT>())
            .find(|offset| !(bytes.as_ptr().addr() + offset).is_multiple_of(align_of::<WINHTTP_ASYNC_RESULT>()))
            .unwrap();
        // SAFETY: pointer::add requires the result to stay inside the same
        // allocation. The offset is below the type's alignment and the array is
        // that much longer than the status structure, so the shifted pointer
        // still addresses a readable, initialized payload of that length.
        let unaligned = unsafe { bytes.as_mut_ptr().add(offset) }.cast();

        // SAFETY: complete requires an installed, not-yet-reclaimed context, a
        // payload readable and initialized for the reported length, no
        // overlapping notification, no outstanding exclusive borrow, and no use
        // of the context after the reclaiming notification. `installed`
        // returned the recorded pointer and only `finish` below reclaims it;
        // the payload is the zeroed local array shifted as computed above, so
        // it is readable and initialized for the reported length even though it
        // is misaligned; this test delivers the notification from its own
        // thread with none other outstanding; and the guard borrows the context
        // only sharedly.
        unsafe {
            complete(
                context,
                WINHTTP_CALLBACK_STATUS_REQUEST_ERROR,
                unaligned,
                status_info_len::<WINHTTP_ASYNC_RESULT>(),
            );
        }

        assert!(matches!(
            drive(future).unwrap(),
            CompletionResult::InvalidStatusInfo {
                status: WINHTTP_CALLBACK_STATUS_REQUEST_ERROR,
                ..
            }
        ));

        // SAFETY: finish requires an installed, not-yet-reclaimed context and
        // no overlapping notification. The delivery above left the context
        // installed and has returned; consuming the guard discharges the
        // obligation forbidding later use of it.
        unsafe {
            finish(guard, context, &contexts, session, &closes);
        }
    }
}
