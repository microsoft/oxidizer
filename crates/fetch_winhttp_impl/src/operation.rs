// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Owns the `WinHTTP` callback-ownership handoff and the per-request
//! operation slot.
//!
//! Everything in this module exists to make the asynchronous `WinHTTP`
//! ownership protocol expressible in safe code (implementation.md section 4).
//! A request handle carries a pointer-sized `dwContext` that `WinHTTP` hands
//! back to every callback (implementation.md section 4.2), so the context
//! allocation must outlive the handle and may only be reclaimed by the final
//! `HANDLE_CLOSING` callback (implementation.md section 4.3). The types here
//! encode that protocol as a chain of moves:
//!
//! - [`ContextPool`] supplies the stable per-transport context storage
//!   (implementation.md section 5).
//! - [`ContextInstallation`] performs the one-time handoff of that storage to
//!   `WinHTTP`.
//! - [`RequestGuard`] holds the resulting close authority.
//! - [`OperationFuture`] holds the request handle for exactly as long as one
//!   asynchronous operation is outstanding, which is what makes "at most one
//!   operation per request" a type-level fact rather than a convention
//!   (implementation.md section 4.1 and section 4.5).
//!
//! The request lifecycle that consumes this module lives in
//! [`crate::request`]; the body readers and writers in [`crate::body`] submit
//! their own operations through the same [`RequestGuard`].

use std::fmt;
use std::mem::ManuallyDrop;
use std::pin::Pin;
use std::ptr::{NonNull, with_exposed_provenance_mut};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use events_once::{Disconnected, RawReceiver};
use plurality::Pool;

use crate::bindings::{Bindings as _, WINHTTP_OPTION_CONTEXT_VALUE};
use crate::context::{ColdConnectState, CompletionResult, OperationBuffer, OperationKind, RequestContext};
use crate::convert::context_bytes;
use crate::error::Result as WinHttpResult;
use crate::handle::{ConnectHandle, RawHandle, RequestHandle};
use crate::session::WinHttpSession;

/// Provides stable callback-context storage for one transport instance.
///
/// Each materialized transport owns a separate pool, so contexts are reused
/// only by requests that share that transport's WinHTTP session. `Pool` is not
/// `Sync`, so the mutex permits allocation and callback-driven return from
/// different threads. The lock is held only while renting or returning an
/// allocation; no WinHTTP call or user code runs while it is held.
pub(crate) type ContextPool = Mutex<Pool<RequestContext>>;

/// Prepares callback ownership for a newly opened request handle.
///
/// The request task initially owns both the RAII request handle and the pooled,
/// pinned context allocation. Successful installation of
/// `WINHTTP_OPTION_CONTEXT_VALUE` transfers context reclamation to the final
/// `HANDLE_CLOSING` callback and returns a [`RequestGuard`] that owns only the
/// request-handle close authority. If installation fails, this type closes the
/// request and reconstructs the pooled box locally.
///
/// This is the ownership handoff, not a configuration step: unlike
/// `RequestSettings` in [`crate::request`], which is a plain value bag of
/// native option values applied to a handle that nothing else refers to yet,
/// installing the context value is the single irreversible moment after which
/// `WinHTTP` may call back into this crate and the context allocation is no
/// longer the request task's to free.
pub(crate) struct ContextInstallation {
    request: RequestHandle,
    context: plurality::Box<RequestContext>,
}

impl ContextInstallation {
    pub(crate) fn new(request: RequestHandle, connect: ConnectHandle, session: Arc<WinHttpSession>, contexts: &ContextPool) -> Self {
        let context = contexts
            .lock()
            .expect("the request-context pool lock cannot be poisoned because no user code runs while it is held")
            .alloc_box(RequestContext::new(connect, session));

        Self { request, context }
    }

    pub(crate) fn install(self) -> WinHttpResult<RequestGuard> {
        let Self { request, context } = self;
        let context = plurality::Box::into_raw(context);
        let mut raw_owner = RawContextOwner::new(context);
        let context_value = context.as_ptr().expose_provenance();
        let option_value = context_bytes(context_value);

        // SAFETY: set_option requires a live handle, the native representation
        // for the option, a valid lifecycle stage, and the trait-level
        // invariants. The RequestHandle owns the sole close authority for a
        // handle nothing has closed; context_bytes produces the pointer-sized
        // value WINHTTP_OPTION_CONTEXT_VALUE takes, in a local that outlives
        // the call; and a request handle accepts that option before its first
        // asynchronous operation, which is this stage, because installation
        // precedes every submission. The trait-level invariants hold for the
        // same reason: the session registered the callback when it opened with
        // WINHTTP_FLAG_ASYNC, this request has no operation outstanding and no
        // buffer lent out, the context was fully initialized before its pointer
        // was exposed, and no borrow of it is held across the call. raw_owner
        // keeps that initialized allocation alive until WinHTTP accepts it.
        if let Err(error) = unsafe {
            request
                .bindings()
                .set_option(request.raw(), WINHTTP_OPTION_CONTEXT_VALUE, &option_value)
        } {
            drop(request);
            drop(raw_owner);
            return Err(error);
        }

        raw_owner.release();

        Ok(RequestGuard {
            request: Some(request),
            context,
        })
    }
}

impl fmt::Debug for ContextInstallation {
    #[cfg_attr(coverage_nightly, coverage(off))] // We have no API contract here.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ContextInstallation")
            .field("request", &self.request)
            .field("context", &self.context)
            .finish()
    }
}

/// Guards the raw context pointer during the installation handoff.
///
/// Extracting the pooled box is necessary to obtain the stable pointer WinHTTP
/// stores. This guard reconstructs that box on every pre-installation exit; it
/// is explicitly released only after WinHTTP accepts the context value.
struct RawContextOwner {
    context: Option<NonNull<RequestContext>>,
}

impl RawContextOwner {
    const fn new(context: NonNull<RequestContext>) -> Self {
        Self { context: Some(context) }
    }

    fn release(&mut self) {
        self.context = None;
    }
}

impl Drop for RawContextOwner {
    fn drop(&mut self) {
        let Some(context) = self.context.take() else {
            return;
        };

        // SAFETY: plurality::Box::from_raw requires the exact pointer returned
        // by into_raw, the same allocator type, and exactly one reconstruction.
        // This guard is constructed with that pointer and is its only owner
        // until installation succeeds, at which point release() empties the
        // field; taking the field above therefore reconstructs the pooled Box
        // at most once, and only while that right still belongs to this crate
        // rather than to the callback.
        drop(unsafe { plurality::Box::<RequestContext>::from_raw(context) });
    }
}

#[derive(Debug)]
/// Owns the close authority for one installed WinHTTP request handle.
///
/// Dropping the guard closes the request exactly once and thereby initiates
/// cancellation of any pending operation. It does not own the context
/// allocation: the final `HANDLE_CLOSING` callback reclaims that allocation and
/// releases the retained connect and session parents.
///
/// Mutable access to this guard is required to submit an operation. The future
/// then owns the request handle itself, leaving this guard unable to submit
/// again even if the future is forgotten. Completion restores the handle only
/// after destroying the receiver endpoint; cancellation destroys the receiver
/// first and then closes the handle.
pub(crate) struct RequestGuard {
    request: Option<RequestHandle>,
    context: NonNull<RequestContext>,
}

impl RequestGuard {
    pub(crate) fn raw(&self) -> RawHandle {
        self.request
            .as_ref()
            .expect("an unfinished OperationFuture prevents RequestGuard reuse")
            .raw()
    }

    #[cfg(test)]
    pub(crate) fn context_ptr(&self) -> *mut RequestContext {
        self.context.as_ptr()
    }

    /// Reports whether an [`OperationFuture`] still holds the request handle.
    ///
    /// A cancelled future never returns the handle, which is what makes the
    /// guard permanently unusable afterwards. Tests outside this module assert
    /// that state without reaching into the private handle slot.
    #[cfg(test)]
    pub(crate) const fn request_handle_taken(&self) -> bool {
        self.request.is_none()
    }

    pub(crate) fn context_value(&self) -> usize {
        self.context.as_ptr().expose_provenance()
    }

    pub(crate) fn cold_connect_state(&self) -> ColdConnectState {
        let _request = self
            .request
            .as_ref()
            .expect("an unfinished OperationFuture prevents RequestGuard context access");
        // SAFETY: NonNull::as_ref requires an aligned, dereferenceable pointer
        // to an initialized value that stays live and free of exclusive
        // references for the borrow. The pointer is the into_raw pointer of the
        // context this guard installed, so it is aligned and initialized; only
        // HANDLE_CLOSING reclaims it, and that cannot have arrived while this
        // guard still owns the unclosed request handle checked above. No
        // exclusive reference to an installed context exists, and this borrow
        // ends with the statement.
        unsafe { self.context.as_ref() }.cold_connect_state()
    }

    pub(crate) fn submit(
        &mut self,
        kind: OperationKind,
        buffer: OperationBuffer,
        submit: impl FnOnce(RawHandle, usize) -> WinHttpResult<()>,
    ) -> OperationFuture<'_> {
        let request = self
            .request
            .take()
            .expect("cancelling or forgetting an OperationFuture prevents RequestGuard reuse");
        let raw = request.raw();

        // SAFETY: NonNull::as_ref requires an aligned, dereferenceable pointer
        // to an initialized value that stays live and free of exclusive
        // references for the borrow. The pointer is the into_raw pointer of the
        // installed context, which only HANDLE_CLOSING reclaims and which the
        // request handle taken above therefore keeps alive, since that
        // notification follows closing the handle this method still owns. No
        // exclusive reference to an installed context exists, and the borrow
        // ends at the arming call below, before the submission runs, as the
        // trait-level invariant on inline completion requires.
        let context = unsafe { self.context.as_ref() };
        // SAFETY: Pin::new_unchecked requires the value never to move again
        // before it is dropped. The context lives in a pooled slot whose
        // address plurality::Box::into_raw fixed until the pointer is handed
        // back, which only the reclaiming callback does, and the value is
        // reached solely through that raw pointer until then.
        let context = unsafe { Pin::new_unchecked(context) };
        // SAFETY: arm requires an idle slot under the caller's exclusive
        // RequestGuard borrow, a previous event that reached its terminal
        // state, and storage pinned until both new endpoints are destroyed.
        // This method holds that exclusive borrow, and taking the request
        // handle above proves no earlier submission is outstanding, because the
        // handle returns to the guard only once the previous OperationFuture's
        // receiver is destroyed, which is exactly what drives the previous
        // event to its terminal state. The pinned storage outlives both
        // endpoints, as HANDLE_CLOSING cannot precede closing the handle this
        // method holds.
        let receiver = unsafe { context.arm(kind, buffer) };

        let submit_result = submit(raw, self.context_value());

        if let Err(error) = submit_result {
            // SAFETY: NonNull::as_ref requires an aligned, dereferenceable
            // pointer to an initialized value that stays live and free of
            // exclusive references for the borrow. The local request handle
            // still keeps the installed context from being reclaimed, and no
            // exclusive reference to it exists. Claiming the operation is
            // atomic, so it yields the payload only if no inline callback
            // already consumed it, and no later operation can be armed before
            // this method returns because the guard's handle slot is empty.
            if let Some(active) = unsafe { self.context.as_ref() }.take_kind(kind) {
                active.completion.send(CompletionResult::error(error, active.buffer));
            }
        }

        OperationFuture {
            receiver: ManuallyDrop::new(receiver),
            receiver_live: true,
            request: Some(request),
            context: self.context_value(),
            request_slot: &mut self.request,
        }
    }

    fn close(&mut self) {
        drop(self.request.take());
    }
}

// SAFETY: Send requires that transferring ownership to another thread is
// sound. The guard holds a RequestHandle, which is Send, and a raw pointer to
// an installed RequestContext, which is Sync and is only ever borrowed
// immutably through that pointer, so the shared access the guard performs is
// sound from whichever thread now owns it. The guard never reclaims the
// allocation, which stays the reclaiming callback's exclusive duty, and moving
// the guard moves the sole request-handle close authority with it, so the
// handle is still closed exactly once and from a single thread.
unsafe impl Send for RequestGuard {}

impl Drop for RequestGuard {
    fn drop(&mut self) {
        self.close();
    }
}

#[derive(Debug)]
/// Owns one request handle while awaiting its callback completion.
///
/// Moving the request handle into this future is the safe-code proof that one
/// request has at most one asynchronous operation outstanding. On completion,
/// the receiver endpoint is destroyed before the handle returns to the guard.
/// On cancellation, the receiver is destroyed before the owned handle closes,
/// allowing `HANDLE_CLOSING` to reclaim the embedded event storage safely.
/// Forgetting the future leaks the handle but leaves the guard unusable.
pub(crate) struct OperationFuture<'guard> {
    receiver: ManuallyDrop<RawReceiver<CompletionResult>>,
    receiver_live: bool,
    request: Option<RequestHandle>,
    context: usize,
    request_slot: &'guard mut Option<RequestHandle>,
}

impl Future for OperationFuture<'_> {
    type Output = std::result::Result<CompletionResult, Disconnected>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // SAFETY: get_unchecked_mut requires that the returned reference is
        // never used to move the pinned value. It is used only to pin-project
        // the receiver in place and to update the ownership fields, whose types
        // are Unpin, so nothing structurally pinned is moved out.
        let this = unsafe { self.get_unchecked_mut() };
        assert!(this.receiver_live, "OperationFuture cannot be polled after completion");
        // SAFETY: Pin::new_unchecked requires the value never to move again
        // before it is dropped. The receiver is reached only through this
        // pinned future, which the projection above does not move it out of,
        // and it is destroyed in place below, so its address is fixed for the
        // remainder of its life.
        let result = unsafe { Pin::new_unchecked(&mut *this.receiver) }.poll(cx);
        if result.is_ready() {
            // SAFETY: ManuallyDrop::drop requires that the value is never used
            // again and is dropped at most once. receiver_live is true here and
            // is cleared immediately afterwards, and both the poll assertion
            // and this future's Drop consult that flag, so no path reaches the
            // receiver after this point.
            unsafe {
                ManuallyDrop::drop(&mut this.receiver);
            }
            this.receiver_live = false;

            let request = this.request.take().expect("a completed OperationFuture retains its request handle");
            debug_assert!(
                this.request_slot.is_none(),
                "request_slot is empty while OperationFuture holds the handle, and nothing else may refill it before completion restores the handle"
            );
            *this.request_slot = Some(request);
        }

        result
    }
}

impl OperationFuture<'_> {
    pub(crate) fn cold_connect_state(&self) -> ColdConnectState {
        let _request = self
            .request
            .as_ref()
            .expect("cold-connect state is available only while an operation is pending");
        let context = NonNull::new(with_exposed_provenance_mut::<RequestContext>(self.context))
            .expect("OperationFuture retains the non-null installed request context");
        // SAFETY: NonNull::as_ref requires an aligned, dereferenceable pointer
        // to an initialized value that stays live and free of exclusive
        // references for the borrow. The address round-trips the exposed
        // provenance of the installed context, so it carries provenance for
        // that allocation and is aligned and initialized; the request handle
        // checked above is still open, so the reclaiming HANDLE_CLOSING
        // notification cannot have run. No exclusive reference to an installed
        // context exists, and the borrow ends with this statement.
        unsafe { context.as_ref() }.cold_connect_state()
    }
}

impl Drop for OperationFuture<'_> {
    fn drop(&mut self) {
        if self.receiver_live {
            // SAFETY: ManuallyDrop::drop requires that the value is never used
            // again and is dropped at most once. Drop runs once per value, and
            // receiver_live proves poll has not already destroyed the receiver.
            unsafe {
                ManuallyDrop::drop(&mut self.receiver);
            }
            self.receiver_live = false;
        }
        // The request field drops after this method. Pending cancellation
        // therefore disconnects the receiver before closing the native handle.
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::panic::{AssertUnwindSafe, RefUnwindSafe, UnwindSafe};
    use std::pin::pin;
    use std::ptr::NonNull;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex, mpsc};
    use std::task::{Context, Poll, Wake, Waker};
    use std::thread;

    use bytesbuf::BytesView;
    use bytesbuf::mem::GlobalPool;
    use plurality::Pool;
    use static_assertions::{assert_impl_all, assert_not_impl_any};
    use windows::Win32::Networking::WinHttp::{
        WINHTTP_CALLBACK_STATUS_HEADERS_AVAILABLE, WINHTTP_CALLBACK_STATUS_READ_COMPLETE, WINHTTP_CALLBACK_STATUS_SENDREQUEST_COMPLETE,
        WINHTTP_CALLBACK_STATUS_WRITE_COMPLETE,
    };

    use super::{ContextInstallation, ContextPool, OperationFuture, RawContextOwner, RequestGuard};
    use crate::context::{CompletionResult, OperationBuffer, OperationKind};
    use crate::error::{WinHttpError, WinHttpOperation};
    use crate::handle::{ConnectHandle, RequestHandle};
    use crate::mocks::{
        CONNECT, REQUEST, bindings, closing, complete, context_pointer, drive, finish, installed, installed_context, raw_handle, session,
        status_info_len,
    };

    assert_impl_all!(ContextPool: Send, Sync, std::fmt::Debug, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(ContextInstallation: Send, std::fmt::Debug, UnwindSafe);
    assert_impl_all!(RequestGuard: Send, std::fmt::Debug);
    // The installation owns the context, whose actual UnsafeCell state prevents safe shared observation.
    assert_not_impl_any!(ContextInstallation: RefUnwindSafe);
    // These pointer owners refer to the callback context's actual UnsafeCell state.
    assert_not_impl_any!(RawContextOwner: UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(RequestGuard: UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(RequestGuard: Sync);
    assert_impl_all!(OperationFuture<'static>: Send, std::fmt::Debug);
    // The future mutably borrows the request-handle slot.
    assert_not_impl_any!(OperationFuture<'static>: UnwindSafe);
    // Shared observation after an unwind cannot mutate the borrowed slot or receiver.
    assert_impl_all!(OperationFuture<'static>: RefUnwindSafe);
    assert_not_impl_any!(OperationFuture<'static>: Sync);

    #[test]
    fn context_option_failure_returns_context_and_closes_each_handle_once() {
        let (facade, closes) = bindings(true);
        let contexts = ContextPool::new(Pool::new());
        let session = session(facade.clone());
        let installation = ContextInstallation::new(
            RequestHandle::new(raw_handle(REQUEST), facade.clone()),
            ConnectHandle::new(raw_handle(CONNECT), facade),
            Arc::clone(&session),
            &contexts,
        );

        assert_eq!(contexts.lock().unwrap().len(), 1);
        let error = installation.install().unwrap_err();

        assert_eq!(error.code(), 12019);
        assert_eq!(contexts.lock().unwrap().len(), 0);
        assert_eq!(closes.request.load(Ordering::SeqCst), 1);
        assert_eq!(closes.connect.load(Ordering::SeqCst), 1);
        assert_eq!(closes.session.load(Ordering::SeqCst), 0);

        drop(session);
        assert_eq!(closes.session.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn installed_context_is_reclaimed_only_by_handle_closing() {
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

        drop(session);
        drop(guard);

        assert_eq!(closes.request.load(Ordering::SeqCst), 1);
        assert_eq!(closes.connect.load(Ordering::SeqCst), 0);
        assert_eq!(closes.session.load(Ordering::SeqCst), 0);
        assert_eq!(contexts.lock().unwrap().len(), 1);

        // SAFETY: closing requires an installed, not-yet-reclaimed context, no
        // overlapping notification, no outstanding exclusive borrow, and no
        // dereference of the pointer or of a guard holding it afterwards.
        // `installed_context` recorded the pointer at installation and no
        // earlier notification reclaimed it; this test submits no operation, so
        // nothing is in flight; nothing borrows the context exclusively; and
        // the guard is dropped above, so neither it nor the pointer is used
        // again.
        unsafe {
            closing(context);
        }

        assert_eq!(contexts.lock().unwrap().len(), 0);
        assert_eq!(closes.connect.load(Ordering::SeqCst), 1);
        assert_eq!(closes.session.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn inline_completion_before_submit_returns_is_observed() {
        let (mut guard, context, contexts, session, closes) = installed();
        let future = guard.submit(OperationKind::SendRequest, OperationBuffer::none(), |_, context_value| {
            assert_eq!(context_value, context.expose_provenance());
            assert_eq!(context_value, closes.context.load(Ordering::SeqCst));
            // SAFETY: complete requires an installed, not-yet-reclaimed
            // context, a payload matching the notification, no overlapping
            // notification, no outstanding exclusive borrow, and no use of the
            // context after the reclaiming notification. `installed` returned
            // the recorded pointer and only `finish` below reclaims it; a send
            // completion carries no payload; this is the only notification the
            // test delivers before awaiting the future; and a submission
            // closure runs with no exclusive borrow of the context
            // outstanding, which is what admits inline completion at all.
            unsafe {
                complete(context, WINHTTP_CALLBACK_STATUS_SENDREQUEST_COMPLETE, std::ptr::null_mut(), 0);
            }
            Ok(())
        });

        assert!(matches!(drive(future).unwrap(), CompletionResult::SendRequestComplete));
        // SAFETY: finish requires an installed, not-yet-reclaimed context and
        // no overlapping notification. The inline delivery above left the
        // context installed and the awaited future proves it has returned;
        // consuming the guard discharges the obligation forbidding later use of
        // it.
        unsafe {
            finish(guard, context, &contexts, session, &closes);
        }
    }

    #[test]
    fn foreign_thread_completion_wakes_send_future() {
        /// Records whether the awaiting future's waker was invoked.
        struct RecordingWaker(AtomicBool);

        impl Wake for RecordingWaker {
            fn wake(self: Arc<Self>) {
                self.0.store(true, Ordering::SeqCst);
            }
        }

        let (mut guard, context, contexts, session, closes) = installed();
        let completer = Arc::new(Mutex::new(None));
        let spawner = Arc::clone(&completer);
        // Gates the foreign delivery behind the test's first poll. Without the
        // gate the completion could land before the future is ever polled, so
        // the first poll would return the result directly and the wake this
        // test exists to observe would never occur.
        let (release, released) = mpsc::channel::<()>();
        let future = guard.submit(OperationKind::HeadersAvailable, OperationBuffer::none(), move |_, context_value| {
            *spawner.lock().unwrap() = Some(thread::spawn(move || {
                let context = context_pointer(context_value);
                released.recv().unwrap();
                // SAFETY: complete requires an installed, not-yet-reclaimed
                // context, a payload matching the notification, no overlapping
                // notification, no outstanding exclusive borrow, and no use of
                // the context after the reclaiming notification. The value
                // handed to the submission closure round-trips the provenance
                // the guard exposed for the installed context, which only
                // `finish` below reclaims, and the test joins this thread
                // before that; a headers-available completion carries no
                // payload; this is the only notification in flight; and a
                // submission runs with no exclusive borrow of the context
                // outstanding.
                unsafe {
                    complete(context, WINHTTP_CALLBACK_STATUS_HEADERS_AVAILABLE, std::ptr::null_mut(), 0);
                }
            }));
            Ok(())
        });

        // The future borrows the guard, so it is confined to a scope that ends
        // before `finish` consumes the guard.
        {
            let mut future = pin!(future);
            let waker_state = Arc::new(RecordingWaker(AtomicBool::new(false)));
            let waker = Waker::from(Arc::clone(&waker_state));
            let mut cx = Context::from_waker(&waker);

            assert!(
                future.as_mut().poll(&mut cx).is_pending(),
                "the gated completion cannot have landed before the first poll"
            );

            release.send(()).unwrap();
            // Waking this future happens partway through the foreign dispatch,
            // so the thread is joined before teardown delivers HANDLE_CLOSING:
            // that notification reclaims the context and may not overlap
            // another notification for the handle.
            completer
                .lock()
                .unwrap()
                .take()
                .expect("the submission closure runs before submit returns and always stores its thread")
                .join()
                .expect("the dispatching thread asserts nothing and so cannot panic");

            assert!(
                waker_state.0.load(Ordering::SeqCst),
                "a completion delivered from a foreign thread must wake the awaiting future"
            );

            let Poll::Ready(completion) = future.as_mut().poll(&mut cx) else {
                panic!("the completion has been delivered, so the future must now be ready")
            };
            assert!(matches!(completion.unwrap(), CompletionResult::HeadersAvailable));
        }

        // SAFETY: finish requires an installed, not-yet-reclaimed context and
        // no overlapping notification. Nothing reclaimed the context, and the
        // join above proves the foreign delivery has returned, so this
        // notification overlaps none; consuming the guard discharges the
        // obligation forbidding later use of it.
        unsafe {
            finish(guard, context, &contexts, session, &closes);
        }
    }

    #[test]
    fn synchronous_submit_failure_completes_once() {
        let (mut guard, context, contexts, session, closes) = installed();
        let future = guard.submit(OperationKind::SendRequest, OperationBuffer::none(), |_, _| {
            Err(WinHttpError::new(12029, WinHttpOperation::SendRequest))
        });

        let CompletionResult::Error { error, _buffer: buffer } = drive(future).unwrap() else {
            panic!("synchronous failure must produce an error completion");
        };
        assert_eq!(error.code(), 12029);
        assert!(buffer.is_none());
        // SAFETY: complete requires an installed, not-yet-reclaimed context, a
        // payload matching the notification, no overlapping notification, no
        // outstanding exclusive borrow, and no use of the context after the
        // reclaiming notification. `installed` returned the recorded pointer
        // and only `finish` below reclaims it; a send completion carries no
        // payload; the failed submission left nothing in flight and this test
        // delivers from its own thread; and the guard borrows the context only
        // sharedly.
        unsafe {
            complete(context, WINHTTP_CALLBACK_STATUS_SENDREQUEST_COMPLETE, std::ptr::null_mut(), 0);
        }
        // SAFETY: finish requires an installed, not-yet-reclaimed context and
        // no overlapping notification. The delivery above left the context
        // installed and has returned; consuming the guard discharges the
        // obligation forbidding later use of it.
        unsafe {
            finish(guard, context, &contexts, session, &closes);
        }
    }

    #[test]
    fn sequential_success_statuses_decode_and_return_buffers() {
        let (mut guard, context, contexts, session, closes) = installed();

        let send = guard.submit(OperationKind::SendRequest, OperationBuffer::none(), |_, _| {
            // SAFETY: complete requires an installed, not-yet-reclaimed
            // context, a payload matching the notification, no overlapping
            // notification, no outstanding exclusive borrow, and no use of the
            // context after the reclaiming notification. `installed` returned
            // the recorded pointer and only `finish` at the end of this test
            // reclaims it; a send completion carries no payload; each
            // submission below is awaited before the next begins, so no two
            // notifications overlap; and a submission runs with no exclusive
            // borrow of the context outstanding.
            unsafe {
                complete(context, WINHTTP_CALLBACK_STATUS_SENDREQUEST_COMPLETE, std::ptr::null_mut(), 0);
            }
            Ok(())
        });
        assert!(matches!(drive(send).unwrap(), CompletionResult::SendRequestComplete));

        let headers = guard.submit(OperationKind::HeadersAvailable, OperationBuffer::none(), |_, _| {
            // SAFETY: as for the send completion above; a headers-available
            // completion likewise carries no payload.
            unsafe {
                complete(context, WINHTTP_CALLBACK_STATUS_HEADERS_AVAILABLE, std::ptr::null_mut(), 0);
            }
            Ok(())
        });
        assert!(matches!(drive(headers).unwrap(), CompletionResult::HeadersAvailable));

        let mut read_memory = [0_u8; 8];
        let read_address = read_memory.as_mut_ptr().addr();
        let read = guard.submit(
            OperationKind::Read,
            OperationBuffer::read(GlobalPool::new().reserve(8), read_address, 8),
            |_, _| {
                // SAFETY: as for the send completion above, except that this
                // notification carries a payload: the address and filled length
                // of the local `read_memory`, which the submission reported as
                // the read buffer and which is initialized and readable for its
                // whole length.
                unsafe {
                    complete(context, WINHTTP_CALLBACK_STATUS_READ_COMPLETE, read_memory.as_mut_ptr().cast(), 5);
                }
                Ok(())
            },
        );
        assert!(matches!(drive(read).unwrap(), CompletionResult::ReadComplete { len: 5, .. }));

        let mut written = 4_u32;
        let write = guard.submit(
            OperationKind::Write,
            OperationBuffer::write(BytesView::copied_from_slice(b"data", &GlobalPool::new()), 4),
            |_, _| {
                // SAFETY: as for the send completion above, except that this
                // notification carries a payload: the initialized local
                // `written`, which outlives the call and which nothing else can
                // reach while the closure borrows it.
                unsafe {
                    complete(
                        context,
                        WINHTTP_CALLBACK_STATUS_WRITE_COMPLETE,
                        (&raw mut written).cast(),
                        status_info_len::<u32>(),
                    );
                }
                Ok(())
            },
        );
        assert!(matches!(drive(write).unwrap(), CompletionResult::WriteComplete { len: 4, .. }));
        // SAFETY: finish requires an installed, not-yet-reclaimed context and
        // no overlapping notification. None of the deliveries above reclaimed
        // the context, and every one of them ran inline within a submission
        // that has since been awaited; consuming the guard discharges the
        // obligation forbidding later use of it.
        unsafe {
            finish(guard, context, &contexts, session, &closes);
        }
    }

    #[test]
    fn cancellation_retains_read_and_write_operations_until_handle_closing() {
        for buffer in [
            OperationBuffer::read(GlobalPool::new().reserve(8), NonNull::<u8>::dangling().as_ptr().addr(), 8),
            OperationBuffer::write(BytesView::copied_from_slice(b"outstanding", &GlobalPool::new()), 11),
        ] {
            let (mut guard, context, contexts, session, closes) = installed();
            let kind = match buffer {
                OperationBuffer::Read { .. } => OperationKind::Read,
                OperationBuffer::Write { .. } => OperationKind::Write,
                OperationBuffer::None => unreachable!("test buffers are read or write"),
            };
            let future = guard.submit(kind, buffer, |_, _| Ok(()));

            assert_eq!(contexts.lock().unwrap().len(), 1);
            drop(future);
            assert!(guard.request.is_none());
            assert_eq!(closes.request.load(Ordering::SeqCst), 1);
            let rejected = std::panic::catch_unwind(AssertUnwindSafe(|| {
                drop(guard.submit(OperationKind::SendRequest, OperationBuffer::none(), |_, _| Ok(())));
            }));
            assert!(rejected.is_err());
            drop(session);
            drop(guard);

            assert_eq!(closes.request.load(Ordering::SeqCst), 1);
            assert_eq!(closes.connect.load(Ordering::SeqCst), 0);
            assert_eq!(closes.session.load(Ordering::SeqCst), 0);
            assert_eq!(contexts.lock().unwrap().len(), 1);

            // SAFETY: closing requires an installed, not-yet-reclaimed context,
            // no overlapping notification, no outstanding exclusive borrow, and
            // no dereference of the pointer or of a guard holding it
            // afterwards. `installed` returned the recorded pointer and nothing
            // reclaimed it; dropping the future closed the request handle, so
            // the cancelled operation can raise no further notification; the
            // guard borrows the context only sharedly; and the guard is dropped
            // above, so neither it nor the pointer is used again.
            unsafe {
                closing(context);
            }

            assert_eq!(contexts.lock().unwrap().len(), 0);
            assert_eq!(closes.connect.load(Ordering::SeqCst), 1);
            assert_eq!(closes.session.load(Ordering::SeqCst), 1);
        }
    }

    #[test]
    fn installed_context_outlives_its_pool_handle() {
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

        drop(contexts);

        drop(session);
        drop(guard);
        // SAFETY: closing requires an installed, not-yet-reclaimed context, no
        // overlapping notification, no outstanding exclusive borrow, and no
        // dereference of the pointer or of a guard holding it afterwards.
        // `installed_context` recorded the pointer at installation and nothing
        // reclaimed it; this test submits no operation, so nothing is in
        // flight; nothing borrows the context exclusively; and the guard is
        // dropped above, so neither it nor the pointer is used again. Dropping
        // the pool first does not invalidate the context, whose allocation the
        // installation keeps alive until this notification reclaims it.
        unsafe {
            closing(context);
        }
        assert_eq!(closes.request.load(Ordering::SeqCst), 1);
        assert_eq!(closes.connect.load(Ordering::SeqCst), 1);
        assert_eq!(closes.session.load(Ordering::SeqCst), 1);
    }
}
