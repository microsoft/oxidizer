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
//!
//! Delivering a notification is an `unsafe` operation here, exactly as it is
//! in [`dispatch_completion`]: the payload the caller describes and the absence
//! of an overlapping notification are properties no harness can inspect. The
//! harness does record which contexts are installed, so that a pointer no
//! installation produced, or one that `HANDLE_CLOSING` already reclaimed, is a
//! panic rather than the undefined behavior it would otherwise be. That check
//! narrows the caller's obligations, it does not discharge them.

use std::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use plurality::Pool;
use testing_aids::FutureTestExt;
use windows::Win32::Networking::WinHttp::{
    ERROR_WINHTTP_INCORRECT_HANDLE_STATE, WINHTTP_ASYNC_RESULT, WINHTTP_CALLBACK_STATUS_HANDLE_CLOSING,
    WINHTTP_CALLBACK_STATUS_REQUEST_ERROR,
};

use crate::bindings::{BindingsFacade, MockBindings, WINHTTP_OPTION_CONTEXT_VALUE};
use crate::callback::dispatch_completion;
use crate::context::RequestContext;
use crate::error::{WinHttpError, WinHttpOperation};
use crate::handle::{ConnectHandle, RawHandle, RequestHandle, SessionHandle};
use crate::operation::{ContextInstallation, ContextPool, RequestGuard};
use crate::session::WinHttpSession;

/// Number of polls [`drive`] spends before declaring a future stalled.
///
/// Mock-backed completions are delivered inline by the binding that submits the
/// operation, so an awaited stage is already complete by the time the future
/// observes it and a whole request lifecycle resolves in a handful of polls.
/// The budget is far above that, which keeps it clear of any legitimate
/// multi-stage sequence while still bounding a stall.
const STALL_POLL_BUDGET: usize = 1024;

/// Drives a mock-backed future to completion without parking the thread.
///
/// Every awaited stage of a request resolves against the operation slot, whose
/// occupant is chosen by the completion-routing tables in
/// [`OperationKind`](crate::context::OperationKind). A defect in that routing
/// leaves the awaited stage forever unanswered, so blocking on such a future
/// would hang the test binary rather than fail it. Polling to a bounded budget
/// turns that stall into an ordinary test failure, which is also what keeps
/// mutation testing of the routing tables terminating.
///
/// Tests that require a completion delivered by another thread cannot use this
/// helper, because no amount of polling substitutes for waiting on that thread.
///
/// # Panics
///
/// Panics if the future is still pending after [`STALL_POLL_BUDGET`] polls.
#[track_caller]
pub(crate) fn drive<F: Future>(future: F) -> F::Output {
    future.unwrap_ready_within(
        STALL_POLL_BUDGET,
        "future never completed: a mock-backed completion was never routed to the stage awaiting it",
    )
}

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
    let context = installed_context(&guard);

    (guard, context, contexts, session, closes)
}

/// Records a guard's context as installed and returns its raw pointer.
///
/// A test that builds its own [`ContextInstallation`] rather than calling
/// [`installed`] must take the pointer from here, because the record is what
/// permits [`complete`] and [`closing`] to dispatch to it.
pub(crate) fn installed_context(guard: &RequestGuard) -> *mut RequestContext {
    record_installed(guard.context_ptr())
}

/// Records a context observed through `WINHTTP_OPTION_CONTEXT_VALUE`.
///
/// A harness whose mock bindings drive a request pipeline it does not own sees
/// the installation only as the pointer-sized value `WinHTTP` is handed, so it
/// registers the context from that value. Call this where the installation is
/// observed rather than where a notification is delivered: registering at
/// delivery would admit every address a caller names and cost the record its
/// purpose.
pub(crate) fn installed_context_value(value: usize) -> *mut RequestContext {
    record_installed(context_pointer(value))
}

/// Rebuilds a context pointer from the address `WinHTTP` was handed.
///
/// A harness that stores what its mock bindings observed holds the context as a
/// plain address and must reconstruct the pointer to dispatch to it. The
/// address round-trips the provenance `RequestGuard` exposed for the context,
/// so the result addresses that allocation rather than carrying no provenance
/// at all.
///
/// This reconstruction alone does not admit a context for dispatch: it must
/// also have been registered, which [`installed_context`] and
/// [`installed_context_value`] do where the installation is observed.
pub(crate) fn context_pointer(value: usize) -> *mut RequestContext {
    std::ptr::with_exposed_provenance_mut(value)
}

fn record_installed(context: *mut RequestContext) -> *mut RequestContext {
    let mut installed = installed_contexts();

    if !installed.contains(&context.addr()) {
        installed.push(context.addr());
    }

    context
}

/// Runs the terminal half of the ownership protocol and asserts it completed.
///
/// Dropping the session owner and the guard closes the request handle, and the
/// subsequent `HANDLE_CLOSING` dispatch is what returns the context to the pool
/// and releases the retained connect and session parents
/// (implementation.md section 4.3). Every handle must be closed exactly once.
///
/// # Safety
///
/// The obligations of [`closing`] apply to the notification this delivers,
/// except that consuming `guard` discharges the one forbidding later use of a
/// guard that still holds the reclaimed context.
pub(crate) unsafe fn finish(
    guard: RequestGuard,
    context: *mut RequestContext,
    contexts: &ContextPool,
    session: Arc<WinHttpSession>,
    closes: &CloseCounts,
) {
    drop(session);
    drop(guard);
    // SAFETY: closing requires the pointer of an installed context whose
    // reclaiming notification has not been delivered, no overlapping
    // notification, no outstanding exclusive borrow, and no later use of the
    // pointer or of a guard holding it. This function's contract demands the
    // first three of its own caller. The guard is dropped above and the pointer
    // is not touched again here, so the last one holds for this call site.
    unsafe {
        closing(context);
    }

    assert_eq!(contexts.lock().unwrap().len(), 0);
    assert_eq!(closes.request.load(Ordering::SeqCst), 1);
    assert_eq!(closes.connect.load(Ordering::SeqCst), 1);
    assert_eq!(closes.session.load(Ordering::SeqCst), 1);
}

/// Delivers one status notification exactly as the `WinHTTP` callback would.
///
/// The pointer is checked against the record of installed contexts before it
/// reaches [`dispatch_completion`], so an address no installation produced, or
/// one a `HANDLE_CLOSING` notification already reclaimed, panics here instead
/// of reconstructing the pooled box twice or reading released storage. The
/// record holds addresses, which say nothing about provenance, so that check
/// narrows the contract below rather than replacing it.
///
/// # Safety
///
/// - `context` must be the pointer [`installed`], [`installed_context`] or
///   [`installed_context_value`] produced, and the `HANDLE_CLOSING`
///   notification for it must not have been delivered yet.
/// - `info` must be null, or readable and initialized for `len` bytes that stay
///   valid and unmodified until this function returns.
///   `WINHTTP_CALLBACK_STATUS_READ_COMPLETE` is exempt: its payload states the
///   address and byte count of the buffer the read filled and is compared
///   against the submitted buffer rather than dereferenced, which is what lets
///   a caller describe bytes no read wrote.
/// - No other notification for the same context may overlap this one, so a
///   caller that dispatches from another thread must join that thread before
///   delivering the next notification.
/// - No exclusive borrow of the request context may be outstanding, because the
///   callback takes its own shared borrow. Shared borrows may be held across
///   the call.
/// - `WINHTTP_CALLBACK_STATUS_HANDLE_CLOSING` reclaims the context allocation,
///   so afterwards neither `context` nor a [`RequestGuard`] that still holds it
///   may be dereferenced. Dropping such a guard remains permitted, because that
///   closes the request handle without reaching the context.
pub(crate) unsafe fn complete(context: *mut RequestContext, status: u32, info: *mut c_void, len: u32) {
    assert!(
        claim(context, status == WINHTTP_CALLBACK_STATUS_HANDLE_CLOSING),
        "the harness delivers notifications only to a context it installed and has not yet reclaimed: \
         take the pointer from `installed`, `installed_context` or `installed_context_value`, and deliver `HANDLE_CLOSING` once"
    );

    // SAFETY: dispatch_completion requires the exact pointer of an installed,
    // not-yet-reclaimed context, a payload matching the notification, a final
    // `HANDLE_CLOSING` that is delivered once and overlaps no other
    // notification for the handle, and no exclusive borrow of the context.
    // Every one of those is demanded of this function's own caller in the same
    // terms, so they hold here unchanged; the claim above additionally proves
    // that the address is one this module saw installed and has not since
    // reclaimed.
    unsafe {
        dispatch_completion(context, status, info, len);
    }
}

/// Claims one dispatch to `context`, releasing the record when it reclaims.
///
/// Reports whether `context` is a context this harness installed and has not
/// yet reclaimed.
fn claim(context: *mut RequestContext, reclaims: bool) -> bool {
    let mut installed = installed_contexts();
    let Some(index) = installed.iter().position(|address| *address == context.addr()) else {
        return false;
    };

    if reclaims {
        installed.swap_remove(index);
    }

    true
}

/// Borrows the record of installed, not-yet-reclaimed request contexts.
///
/// Addresses are recorded rather than pointers because the record outlives the
/// allocations it names, and an address is all a claim needs to compare.
fn installed_contexts() -> MutexGuard<'static, Vec<usize>> {
    /// The dispatch helpers cannot verify a raw context pointer, so they check
    /// it against this record and turn the misuse a test is most likely to
    /// commit into a panic. It is process-wide because a context pointer
    /// travels to callback threads and into mock bindings that hold no harness
    /// state.
    static INSTALLED_CONTEXTS: Mutex<Vec<usize>> = Mutex::new(Vec::new());

    INSTALLED_CONTEXTS
        .lock()
        .expect("the harness runs nothing that can panic while this lock is held, so it cannot be poisoned")
}

/// Reports the native status-info length for a status payload of type `T`.
pub(crate) fn status_info_len<T>() -> u32 {
    u32::try_from(size_of::<T>()).unwrap()
}

/// Delivers the final `HANDLE_CLOSING` notification for an installed context.
///
/// # Safety
///
/// The obligations of [`complete`] apply, except the one describing the
/// payload: this notification carries none. It is the notification that
/// reclaims the pooled allocation, so afterwards neither `context` nor a
/// [`RequestGuard`] that still holds it may be dereferenced.
pub(crate) unsafe fn closing(context: *mut RequestContext) {
    // SAFETY: complete requires an installed, not-yet-reclaimed context, a
    // payload matching the notification, no overlapping notification, no
    // outstanding exclusive borrow, and no use of the context after the
    // reclaiming notification. All but the payload are demanded of this
    // function's own caller in the same terms. `HANDLE_CLOSING` carries no
    // payload, and a null pointer of zero length is how `WinHTTP` states that.
    unsafe {
        complete(context, WINHTTP_CALLBACK_STATUS_HANDLE_CLOSING, std::ptr::null_mut(), 0);
    }
}

/// Delivers a `REQUEST_ERROR` notification carrying `code`.
///
/// The payload lives on this function's own stack frame for the duration of the
/// dispatch, which is how the notification's payload obligation is discharged
/// for every caller at once.
///
/// # Safety
///
/// The obligations of [`complete`] apply, except the one describing the
/// payload, which this function supplies.
pub(crate) unsafe fn complete_request_error(context: *mut RequestContext, code: u32) {
    let mut result = WINHTTP_ASYNC_RESULT {
        dwResult: 0,
        dwError: code,
    };

    // SAFETY: complete requires an installed, not-yet-reclaimed context, a
    // payload readable and unmodified for the call, no overlapping
    // notification, no outstanding exclusive borrow, and no use of the context
    // after the reclaiming notification. All but the payload are demanded of
    // this function's own caller in the same terms. The payload is the local
    // above, which `WINHTTP_ASYNC_RESULT` initializes in full, lives until this
    // function returns, and nothing else can reach.
    unsafe {
        complete(
            context,
            WINHTTP_CALLBACK_STATUS_REQUEST_ERROR,
            (&raw mut result).cast(),
            status_info_len::<WINHTTP_ASYNC_RESULT>(),
        );
    }
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
                // Models a SetOption call rejected because the handle is not in
                // a state that accepts it.
                Err(WinHttpError::new(ERROR_WINHTTP_INCORRECT_HANDLE_STATE, WinHttpOperation::SetOption))
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
