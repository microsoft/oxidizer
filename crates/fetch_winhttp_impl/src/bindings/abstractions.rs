// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::ffi::c_void;
use std::ptr::NonNull;

use widestring::U16CStr;

use crate::error::Result;
use crate::handle::RawHandle;

/// Callback ABI accepted by WinHTTP, with `None` clearing the callback.
pub(crate) type StatusCallback = Option<unsafe extern "system" fn(*mut c_void, usize, u32, *mut c_void, u32)>;

/// Defines the WinHTTP boundary used by production code and deterministic tests.
///
/// Implementations provide only the WinHTTP operations the transport needs.
///
/// # Safety
///
/// Implementations must preserve WinHTTP-compatible handle, callback, and
/// completion semantics. Callers must preserve these cross-method invariants in
/// addition to each method's specific requirements:
///
/// - The status callback is registered and the fully initialized request context
///   is installed before the first asynchronous submission.
/// - Every session is opened with `WINHTTP_FLAG_ASYNC`, and all child handles
///   inherit that asynchronous callback behavior.
/// - No exclusive borrow of the request context is outstanding across a
///   submission, because WinHTTP may complete inline and reenter the callback
///   on the submitting thread, where the callback takes its own shared borrow.
///   Shared borrows may be held across a submission; interior mutability
///   carries every state change the callback and the submitter share.
/// - At most one asynchronous operation is outstanding per request handle.
/// - A lent buffer remains retained until one of the endpoints named by the
///   submitting method ends the operation: the matching completion notification
///   (`READ_COMPLETE` or `WRITE_COMPLETE`), `REQUEST_ERROR`, or the request
///   handle's final `HANDLE_CLOSING` callback. A failing return from the
///   submitting call is not one of those endpoints: it starts no operation, so
///   no buffer was ever lent and the caller reclaims it immediately.
/// - RAII handle owners close each successfully created handle exactly once; the
///   final handle-closing callback is the terminal context-ownership event.
///
/// Production bindings preserve native WinHTTP behavior, while mocks must model
/// these same ordering and lifetime rules for deterministic tests to be sound.
#[cfg_attr(test, mockall::automock)]
pub(crate) unsafe trait Bindings: Send + Sync + 'static {
    /// Opens an asynchronous WinHTTP session.
    ///
    /// # Safety
    ///
    /// The caller must satisfy all trait-level safety requirements. `flags`
    /// must include `WINHTTP_FLAG_ASYNC`. A returned handle must immediately
    /// acquire one owner that closes it exactly once.
    unsafe fn open(&self, user_agent: &U16CStr, flags: u32) -> Result<RawHandle>;

    /// Applies the four native resolve/connect/send/receive timeouts.
    ///
    /// # Safety
    ///
    /// The caller must satisfy all trait-level safety requirements. `handle`
    /// must identify a live session that cannot close during this call.
    unsafe fn set_timeouts(&self, handle: RawHandle, resolve: i32, connect: i32, send: i32, receive: i32) -> Result<()>;

    /// Registers the session status callback and its notification mask.
    ///
    /// # Safety
    ///
    /// The caller must satisfy all trait-level safety requirements. `handle`
    /// must identify a live session. `callback` must be present and remain
    /// valid for the session lifetime, and `notification_flags` must enable
    /// every completion, error, diagnostic, and final handle-closing status
    /// required by the callback protocol. Registration must precede every
    /// child request that relies on that protocol.
    unsafe fn set_status_callback(&self, handle: RawHandle, callback: StatusCallback, notification_flags: u32) -> Result<()>;

    /// Opens a connect handle for one host and port under a live session.
    ///
    /// # Safety
    ///
    /// The caller must satisfy all trait-level safety requirements. `session`
    /// must remain live until the returned child handle is closed. A returned
    /// handle must immediately acquire one owner that closes it exactly once.
    unsafe fn connect(&self, session: RawHandle, host: &U16CStr, port: u16) -> Result<RawHandle>;

    /// Opens a request handle under a live connect handle.
    ///
    /// # Safety
    ///
    /// The caller must satisfy all trait-level safety requirements. `connect`
    /// must remain live until the returned request is finally closed. A
    /// returned handle must immediately acquire one exactly-once owner.
    unsafe fn open_request(&self, connect: RawHandle, method: &U16CStr, path: &U16CStr, flags: u32) -> Result<RawHandle>;

    /// Writes one WinHTTP option onto a live handle.
    ///
    /// # Safety
    ///
    /// The caller must satisfy all trait-level safety requirements. `handle`
    /// must remain live, `value` must use the native representation required by
    /// `option`, and the option must be applied at a valid lifecycle stage.
    unsafe fn set_option(&self, handle: RawHandle, option: u32, value: &[u8]) -> Result<()>;

    /// Submits request headers and begins the asynchronous send operation.
    ///
    /// # Safety
    ///
    /// The caller must satisfy all trait-level safety requirements. `request`
    /// must identify a live request with its operation slot armed for
    /// `SendRequest`. `context` must be the exact context pointer already
    /// installed on the request, and that context must remain valid until the
    /// request handle's final `HANDLE_CLOSING` callback.
    unsafe fn send_request(&self, request: RawHandle, headers: &U16CStr, total_len: u32, context: usize) -> Result<()>;

    /// Writes one contiguous request-body span, or the terminal zero-length write.
    ///
    /// # Safety
    ///
    /// The caller must satisfy all trait-level safety requirements. `request`
    /// must identify a live request with its operation slot armed for write.
    /// A present `buffer` must be readable for `len` initialized bytes and
    /// remain valid and unchanged until `WRITE_COMPLETE`, `REQUEST_ERROR`, the
    /// request handle's final `HANDLE_CLOSING` callback, or a failing return
    /// from this call terminates the operation. An absent buffer is valid only
    /// for the zero-length write that ends automatic chunking.
    ///
    /// A failing return obliges the implementation to have started no write and
    /// to retain no reference to `buffer`, because the caller reclaims the
    /// buffer immediately and no completion callback follows.
    /// [`WinHttpWriteData`] satisfies this by initiating no operation when it
    /// reports failure.
    ///
    /// [`WinHttpWriteData`]: https://learn.microsoft.com/windows/win32/api/winhttp/nf-winhttp-winhttpwritedata
    unsafe fn write_data(&self, request: RawHandle, buffer: Option<NonNull<u8>>, len: u32) -> Result<()>;

    /// Starts receiving response headers after the request body is complete.
    ///
    /// # Safety
    ///
    /// The caller must satisfy all trait-level safety requirements. `request`
    /// must be live, request-body submission must be complete, and its
    /// operation slot must be armed for response headers.
    unsafe fn receive_response(&self, request: RawHandle) -> Result<()>;

    /// Queries response headers or related status metadata from a live request.
    ///
    /// # Safety
    ///
    /// The caller must satisfy all trait-level safety requirements. `request`
    /// must be live with no asynchronous operation outstanding.
    /// `buffer` must be writable for `*buffer_len` bytes when present.
    unsafe fn query_headers(&self, request: RawHandle, info_level: u32, buffer: Option<NonNull<u8>>, buffer_len: &mut u32) -> Result<()>;

    /// Queries one WinHTTP option from a live handle.
    ///
    /// # Safety
    ///
    /// The caller must satisfy all trait-level safety requirements. `handle`
    /// must be live with no asynchronous operation outstanding.
    /// `buffer` must be writable for `*buffer_len` bytes when present.
    unsafe fn query_option(&self, handle: RawHandle, option: u32, buffer: Option<NonNull<u8>>, buffer_len: &mut u32) -> Result<()>;

    /// Asks how many response-body bytes are ready to read.
    ///
    /// # Safety
    ///
    /// The caller must satisfy all trait-level safety requirements. `request`
    /// must be live with its operation slot armed for data availability.
    unsafe fn query_data_available(&self, request: RawHandle) -> Result<()>;

    /// Reads response-body bytes into a caller-owned buffer.
    ///
    /// # Safety
    ///
    /// The caller must satisfy all trait-level safety requirements. `request`
    /// must be live with its operation slot armed for reading.
    /// `buffer` must be writable for `len` bytes and remain valid and untouched
    /// until `READ_COMPLETE`, `REQUEST_ERROR`, the request handle's final
    /// `HANDLE_CLOSING` callback, or a failing return from this call terminates
    /// the operation.
    ///
    /// A failing return obliges the implementation to have started no read and
    /// to retain no reference to `buffer`, because the caller reclaims the
    /// buffer immediately and no completion callback follows.
    /// [`WinHttpReadData`] satisfies this by initiating no operation when it
    /// reports failure.
    ///
    /// [`WinHttpReadData`]: https://learn.microsoft.com/windows/win32/api/winhttp/nf-winhttp-winhttpreaddata
    unsafe fn read_data(&self, request: RawHandle, buffer: NonNull<u8>, len: u32) -> Result<()>;

    /// Closes a handle owned by the caller.
    ///
    /// # Safety
    ///
    /// The caller must satisfy all trait-level safety requirements and own the
    /// sole close authority for `handle`. No later API call may use the handle,
    /// and an installed context must remain valid through `HANDLE_CLOSING`.
    unsafe fn close_handle(&self, handle: RawHandle) -> Result<()>;
}
