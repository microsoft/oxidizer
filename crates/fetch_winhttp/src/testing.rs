// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Builds a live, mock-backed request context for this crate's own unit tests.
//!
//! Exercising the callback-ownership protocol (implementation.md section 4)
//! requires a fully installed request context: a mock session handle, a mock
//! connect handle, a pooled [`RequestContext`], and a
//! [`RequestGuard`](crate::operation::RequestGuard) whose raw context pointer
//! can be handed to [`dispatch_completion`] exactly as `WinHTTP` would. That
//! setup is identical for every test that drives a callback, but the modules
//! that need it - [`crate::operation`] for the handoff itself,
//! [`crate::context`] for the state a callback records, and
//! [`crate::callback`] for dispatch and the operation slot's atomic ownership
//! transfer - are three separate modules, so the harness lives here rather
//! than being duplicated or arbitrarily hosted by one of them.
//!
//! The module is `#[cfg(test)]`-gated and crate-private, unlike the public
//! `testing` modules elsewhere in this workspace: it depends on
//! [`BindingsFacade::mock`], which is itself `#[cfg(test)]`-gated, so it cannot
//! be compiled into a shipped build or consumed from `tests/`.
//!
//! The mock bindings count `WinHttpCloseHandle` calls per handle, which is what
//! lets a test assert the exactly-once closure and the ordered release of the
//! session before the connect handle that `HANDLE_CLOSING` performs
//! (implementation.md section 4.3).

use std::ffi::c_void;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use plurality::Pool;
use windows::Win32::Networking::WinHttp::WINHTTP_CALLBACK_STATUS_HANDLE_CLOSING;

use crate::bindings::{BindingsFacade, MockBindings, WINHTTP_OPTION_CONTEXT_VALUE};
use crate::callback::dispatch_completion;
use crate::context::RequestContext;
use crate::error::{WinHttpError, WinHttpOperation};
use crate::handle::{ConnectHandle, RawHandle, RequestHandle, SessionHandle};
use crate::operation::{ContextInstallation, ContextPool, RequestGuard};
use crate::session::WinHttpSession;

/// Address used for the mock session handle.
///
/// The three handle constants are distinct nonzero addresses so that the
/// close-counting mock can attribute each `WinHttpCloseHandle` call to the
/// handle it closed. They are never dereferenced.
pub(crate) const SESSION: usize = 1;
/// Address used for the mock connect handle. See [`SESSION`].
pub(crate) const CONNECT: usize = 2;
/// Address used for the mock request handle. See [`SESSION`].
pub(crate) const REQUEST: usize = 3;

/// Records what the mock bindings observed for one request lifecycle.
///
/// Handle closure is the observable half of the ownership protocol, so tests
/// assert against these counters rather than against internal state: the
/// request closes when its guard drops, while the connect and session parents
/// close only when `HANDLE_CLOSING` reclaims the context. `context` retains the
/// pointer-sized value that was installed through
/// `WINHTTP_OPTION_CONTEXT_VALUE`, so a test can prove the value `WinHTTP`
/// received is the same one the callback is later invoked with.
#[derive(Default)]
pub(crate) struct CloseCounts {
    pub(crate) context: AtomicUsize,
    pub(crate) session: AtomicUsize,
    pub(crate) connect: AtomicUsize,
    pub(crate) request: AtomicUsize,
}

/// Installs a context on a mock request handle and returns the live protocol state.
///
/// The returned guard, raw context pointer, pool, session owner, and close
/// counters are exactly the values a request task holds between installation
/// and completion, so a test can submit operations and dispatch callbacks
/// against them. Pass all five to [`finish`] to run the teardown assertions.
pub(crate) fn installed() -> (
    RequestGuard,
    *mut RequestContext,
    ContextPool,
    Arc<WinHttpSession>,
    Arc<CloseCounts>,
) {
    let (facade, closes) = bindings(false);
    let contexts = ContextPool::new(Pool::new());
    let session = session(facade.clone());
    let guard = ContextInstallation::new(
        RequestHandle::new(raw_handle(REQUEST), facade.clone()),
        ConnectHandle::new(raw_handle(CONNECT), facade),
        Arc::clone(&session),
        &contexts,
    )
    .install()
    .unwrap();
    let context = guard.context_ptr();

    (guard, context, contexts, session, closes)
}

/// Runs the terminal half of the ownership protocol and asserts it completed.
///
/// Dropping the session owner and the guard closes the request handle, and the
/// subsequent `HANDLE_CLOSING` dispatch is what returns the context to the pool
/// and releases the retained connect and session parents
/// (implementation.md section 4.3). Every handle must be closed exactly once.
pub(crate) fn finish(
    guard: RequestGuard,
    context: *mut RequestContext,
    contexts: &ContextPool,
    session: Arc<WinHttpSession>,
    closes: &CloseCounts,
) {
    drop(session);
    drop(guard);
    closing(context);

    assert_eq!(contexts.lock().unwrap().len(), 0);
    assert_eq!(closes.request.load(Ordering::SeqCst), 1);
    assert_eq!(closes.connect.load(Ordering::SeqCst), 1);
    assert_eq!(closes.session.load(Ordering::SeqCst), 1);
}

/// Delivers one status notification exactly as the `WinHTTP` callback would.
pub(crate) fn complete(context: *mut RequestContext, status: u32, info: *mut c_void, len: u32) {
    // SAFETY: every test passes a live installed context and preserves each
    // status-info object for the duration of the synchronous dispatch.
    unsafe {
        dispatch_completion(context, status, info, len);
    }
}

/// Reports the native status-info length for a status payload of type `T`.
pub(crate) fn status_info_len<T>() -> u32 {
    u32::try_from(size_of::<T>()).unwrap()
}

/// Delivers the final `HANDLE_CLOSING` notification for an installed context.
pub(crate) fn closing(context: *mut RequestContext) {
    complete(context, WINHTTP_CALLBACK_STATUS_HANDLE_CLOSING, std::ptr::null_mut(), 0);
}

/// Wraps a mock session handle in the owner that installed contexts retain.
pub(crate) fn session(facade: BindingsFacade) -> Arc<WinHttpSession> {
    Arc::new(WinHttpSession::from_handle(SessionHandle::new(raw_handle(SESSION), facade)))
}

/// Builds mock bindings that accept or reject the context-value installation.
///
/// `fail_context_option` drives the one failure mode that must not leak: when
/// `WinHttpSetOption` rejects `WINHTTP_OPTION_CONTEXT_VALUE`, `WinHTTP` never
/// takes ownership, so the request task remains responsible for reclaiming the
/// context and closing both handles itself.
pub(crate) fn bindings(fail_context_option: bool) -> (BindingsFacade, Arc<CloseCounts>) {
    let closes = Arc::new(CloseCounts::default());
    let mut bindings = MockBindings::new();
    let context_counts = Arc::clone(&closes);
    bindings
        .expect_set_option()
        .withf(|handle, option, value| {
            *handle == raw_handle(REQUEST)
                && *option == WINHTTP_OPTION_CONTEXT_VALUE
                && value.len() == size_of::<usize>()
                && usize::from_ne_bytes(value.try_into().unwrap()) != 0
        })
        .once()
        .returning(move |_, _, value| {
            context_counts
                .context
                .store(usize::from_ne_bytes(value.try_into().unwrap()), Ordering::SeqCst);
            if fail_context_option {
                Err(WinHttpError::new(12019, WinHttpOperation::SetOption))
            } else {
                Ok(())
            }
        });
    let close_counts = Arc::clone(&closes);
    bindings.expect_close_handle().times(3).returning(move |handle| {
        match handle.as_ptr().addr() {
            SESSION => &close_counts.session,
            CONNECT => &close_counts.connect,
            REQUEST => &close_counts.request,
            _ => panic!("unexpected test handle"),
        }
        .fetch_add(1, Ordering::SeqCst);
        Ok(())
    });

    (BindingsFacade::mock(Arc::new(bindings)), closes)
}

/// Builds a raw handle from one of the fixed test addresses.
pub(crate) fn raw_handle(value: usize) -> RawHandle {
    RawHandle::new(std::ptr::without_provenance_mut::<c_void>(value)).unwrap()
}
