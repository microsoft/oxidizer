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

        // SAFETY: the request is live and has not submitted an asynchronous
        // operation. option_value is the exact pointer-sized context
        // representation, and raw_owner keeps the initialized context alive
        // until WinHTTP accepts ownership.
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

        // SAFETY: this guard uniquely owns the exact pointer returned by
        // plurality::Box::into_raw and reconstructs it only on installation
        // failure, before WinHTTP takes callback ownership.
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
        // SAFETY: the guard remains alive, so callback ownership keeps the
        // installed context valid while it owns the request handle.
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

        // SAFETY: the context was installed from stable pooled storage. The
        // returned future leaves the request-handle slot empty until its
        // receiver endpoint has been destroyed.
        let context = unsafe { self.context.as_ref() };
        // SAFETY: the pooled context has a stable address until
        // HANDLE_CLOSING reclaims it.
        let context = unsafe { Pin::new_unchecked(context) };
        // SAFETY: the context remains pinned until HANDLE_CLOSING reclaims it,
        // which outlives both endpoints of the event armed here. The slot is
        // idle: taking the request handle above proves no earlier submission is
        // outstanding, because the handle only returns to the guard once the
        // previous OperationFuture's receiver is destroyed, and destroying that
        // receiver is exactly what drives the previous event to its terminal
        // state.
        let receiver = unsafe { context.arm(kind, buffer) };

        let submit_result = submit(raw, self.context_value());

        if let Err(error) = submit_result {
            // SAFETY: the local request handle keeps the context valid. The
            // operation kind atomically wins only if no inline callback
            // already consumed the operation, so synchronous failure cannot
            // double-complete it. No later operation can begin before this
            // method returns because the guard's handle slot remains empty.
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

// SAFETY: moving the guard transfers the sole request-handle close authority.
// The context pointer remains valid independently through HANDLE_CLOSING.
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
        // SAFETY: this mutable reference is used only to pin-project the
        // receiver and update unpinned ownership fields; the receiver is not
        // moved.
        let this = unsafe { self.get_unchecked_mut() };
        assert!(this.receiver_live, "OperationFuture cannot be polled after completion");
        // SAFETY: the receiver remains in place for the lifetime of this pinned
        // OperationFuture.
        let result = unsafe { Pin::new_unchecked(&mut *this.receiver) }.poll(cx);
        if result.is_ready() {
            // SAFETY: the receiver is pinned in place and will not be accessed
            // again after receiver_live is cleared.
            unsafe {
                ManuallyDrop::drop(&mut this.receiver);
            }
            this.receiver_live = false;

            let request = this.request.take().expect("a completed OperationFuture retains its request handle");
            debug_assert!(this.request_slot.is_none());
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
        // SAFETY: this future owns the live request handle, so callback
        // ownership keeps the installed context valid.
        unsafe { context.as_ref() }.cold_connect_state()
    }
}

impl Drop for OperationFuture<'_> {
    fn drop(&mut self) {
        if self.receiver_live {
            // SAFETY: Drop runs exactly once, and receiver_live proves the
            // manually managed receiver has not already been destroyed.
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
mod tests {
    use std::panic::{AssertUnwindSafe, RefUnwindSafe, UnwindSafe};
    use std::ptr::NonNull;
    use std::sync::Arc;
    use std::sync::atomic::Ordering;
    use std::thread;

    use bytesbuf::BytesView;
    use bytesbuf::mem::GlobalPool;
    use plurality::Pool;
    use static_assertions::{assert_impl_all, assert_not_impl_any};
    use windows::Win32::Networking::WinHttp::{
        WINHTTP_CALLBACK_STATUS_DATA_AVAILABLE, WINHTTP_CALLBACK_STATUS_HEADERS_AVAILABLE, WINHTTP_CALLBACK_STATUS_READ_COMPLETE,
        WINHTTP_CALLBACK_STATUS_SENDREQUEST_COMPLETE, WINHTTP_CALLBACK_STATUS_WRITE_COMPLETE,
    };

    use super::{ContextInstallation, ContextPool, OperationFuture, RawContextOwner, RequestGuard};
    use crate::context::{CompletionResult, OperationBuffer, OperationKind};
    use crate::error::{WinHttpError, WinHttpOperation};
    use crate::handle::{ConnectHandle, RequestHandle};
    use crate::testing::{CONNECT, REQUEST, bindings, closing, complete, finish, installed, raw_handle, session, status_info_len};

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
        let context = guard.context_ptr();

        drop(session);
        drop(guard);

        assert_eq!(closes.request.load(Ordering::SeqCst), 1);
        assert_eq!(closes.connect.load(Ordering::SeqCst), 0);
        assert_eq!(closes.session.load(Ordering::SeqCst), 0);
        assert_eq!(contexts.lock().unwrap().len(), 1);

        closing(context);

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
            complete(context, WINHTTP_CALLBACK_STATUS_SENDREQUEST_COMPLETE, std::ptr::null_mut(), 0);
            Ok(())
        });

        assert!(matches!(
            futures::executor::block_on(future).unwrap(),
            CompletionResult::SendRequestComplete
        ));
        finish(guard, context, &contexts, session, &closes);
    }

    #[test]
    fn foreign_thread_completion_wakes_send_future() {
        let (mut guard, context, contexts, session, closes) = installed();
        let future = guard.submit(OperationKind::HeadersAvailable, OperationBuffer::none(), |_, context_value| {
            thread::spawn(move || {
                let context = std::ptr::with_exposed_provenance_mut(context_value);
                complete(context, WINHTTP_CALLBACK_STATUS_HEADERS_AVAILABLE, std::ptr::null_mut(), 0);
            });
            Ok(())
        });

        assert!(matches!(
            futures::executor::block_on(future).unwrap(),
            CompletionResult::HeadersAvailable
        ));
        finish(guard, context, &contexts, session, &closes);
    }

    #[test]
    fn synchronous_submit_failure_completes_once() {
        let (mut guard, context, contexts, session, closes) = installed();
        let future = guard.submit(OperationKind::SendRequest, OperationBuffer::none(), |_, _| {
            Err(WinHttpError::new(12029, WinHttpOperation::SendRequest))
        });

        let CompletionResult::Error { error, _buffer: buffer } = futures::executor::block_on(future).unwrap() else {
            panic!("synchronous failure must produce an error completion");
        };
        assert_eq!(error.code(), 12029);
        assert!(buffer.is_none());
        complete(context, WINHTTP_CALLBACK_STATUS_SENDREQUEST_COMPLETE, std::ptr::null_mut(), 0);
        finish(guard, context, &contexts, session, &closes);
    }

    #[test]
    fn sequential_success_statuses_decode_and_return_buffers() {
        let (mut guard, context, contexts, session, closes) = installed();

        let send = guard.submit(OperationKind::SendRequest, OperationBuffer::none(), |_, _| {
            complete(context, WINHTTP_CALLBACK_STATUS_SENDREQUEST_COMPLETE, std::ptr::null_mut(), 0);
            Ok(())
        });
        assert!(matches!(
            futures::executor::block_on(send).unwrap(),
            CompletionResult::SendRequestComplete
        ));

        let headers = guard.submit(OperationKind::HeadersAvailable, OperationBuffer::none(), |_, _| {
            complete(context, WINHTTP_CALLBACK_STATUS_HEADERS_AVAILABLE, std::ptr::null_mut(), 0);
            Ok(())
        });
        assert!(matches!(
            futures::executor::block_on(headers).unwrap(),
            CompletionResult::HeadersAvailable
        ));

        let mut available = 17_u32;
        let data = guard.submit(OperationKind::DataAvailable, OperationBuffer::none(), |_, _| {
            complete(
                context,
                WINHTTP_CALLBACK_STATUS_DATA_AVAILABLE,
                (&raw mut available).cast(),
                status_info_len::<u32>(),
            );
            Ok(())
        });
        assert!(matches!(
            futures::executor::block_on(data).unwrap(),
            CompletionResult::DataAvailable(17)
        ));

        let mut read_memory = [0_u8; 8];
        let read_address = read_memory.as_mut_ptr().addr();
        let read = guard.submit(
            OperationKind::Read,
            OperationBuffer::read(GlobalPool::new().reserve(8), read_address, 8),
            |_, _| {
                complete(context, WINHTTP_CALLBACK_STATUS_READ_COMPLETE, read_memory.as_mut_ptr().cast(), 5);
                Ok(())
            },
        );
        assert!(matches!(
            futures::executor::block_on(read).unwrap(),
            CompletionResult::ReadComplete { len: 5, .. }
        ));

        let mut written = 4_u32;
        let write = guard.submit(
            OperationKind::Write,
            OperationBuffer::write(BytesView::copied_from_slice(b"data", &GlobalPool::new()), 4),
            |_, _| {
                complete(
                    context,
                    WINHTTP_CALLBACK_STATUS_WRITE_COMPLETE,
                    (&raw mut written).cast(),
                    status_info_len::<u32>(),
                );
                Ok(())
            },
        );
        assert!(matches!(
            futures::executor::block_on(write).unwrap(),
            CompletionResult::WriteComplete { len: 4, .. }
        ));
        finish(guard, context, &contexts, session, &closes);
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

            closing(context);

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
        let context = guard.context_ptr();

        drop(contexts);

        drop(session);
        drop(guard);
        closing(context);
        assert_eq!(closes.request.load(Ordering::SeqCst), 1);
        assert_eq!(closes.connect.load(Ordering::SeqCst), 1);
        assert_eq!(closes.session.load(Ordering::SeqCst), 1);
    }
}
