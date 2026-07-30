// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt;
use std::marker::PhantomData;
use std::pin::Pin;
use std::ptr::NonNull;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use events_once::{Disconnected, RawReceiver};
use plurality::Pool;

use crate::bindings::Bindings as _;
use crate::context::{CompletionResult, OperationAlreadyActive, OperationBuffer, OperationKind, RequestContext};
use crate::error::Result;
use crate::handle::{ConnectHandle, RawHandle, RequestHandle};
use crate::options::{WINHTTP_OPTION_CONTEXT_VALUE, context_bytes};
use crate::session::WinHttpSession;

pub(crate) type ContextPool = Mutex<Pool<RequestContext>>;

pub(crate) struct RequestSetup {
    request: RequestHandle,
    context: plurality::Box<RequestContext>,
}

impl RequestSetup {
    pub(crate) fn new(request: RequestHandle, connect: ConnectHandle, session: Arc<WinHttpSession>, contexts: &ContextPool) -> Self {
        let context = contexts
            .lock()
            .expect("the request-context pool lock cannot be poisoned because no user code runs while it is held")
            .alloc_box(RequestContext::new(connect, session));

        Self { request, context }
    }

    pub(crate) fn install(self) -> Result<RequestGuard> {
        let Self { request, context } = self;
        let context = plurality::Box::into_raw(context);
        let mut raw_owner = RawContextOwner::new(context);
        let context_value = context.as_ptr().expose_provenance();
        let option_value = context_bytes(context_value);

        if let Err(error) = request
            .bindings()
            .set_option(request.raw(), WINHTTP_OPTION_CONTEXT_VALUE, &option_value)
        {
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

impl fmt::Debug for RequestSetup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RequestSetup")
            .field("request", &self.request)
            .field("context", &self.context)
            .finish()
    }
}

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
pub(crate) struct RequestGuard {
    request: Option<RequestHandle>,
    context: NonNull<RequestContext>,
}

impl RequestGuard {
    pub(crate) fn raw(&self) -> RawHandle {
        self.request
            .as_ref()
            .expect("RequestGuard retains its request handle until Drop")
            .raw()
    }

    pub(crate) fn context_ptr(&self) -> *mut RequestContext {
        self.context.as_ptr()
    }

    pub(crate) fn context_value(&self) -> usize {
        self.context.as_ptr().expose_provenance()
    }

    pub(crate) fn submit(
        &mut self,
        kind: OperationKind,
        buffer: OperationBuffer,
        submit: impl FnOnce(RawHandle, usize) -> Result<()>,
    ) -> std::result::Result<OperationFuture<'_>, OperationAlreadyActive> {
        // SAFETY: the context was installed from stable pooled storage. The
        // returned future mutably borrows this guard, preventing another
        // operation or request close until its receiver is dropped.
        let context = unsafe { self.context.as_ref() };
        // SAFETY: the pooled context has a stable address until
        // HANDLE_CLOSING reclaims it.
        let context = unsafe { Pin::new_unchecked(context) };
        // SAFETY: the context remains pinned while both embedded-event
        // endpoints exist, as described above.
        let (token, receiver) = unsafe { context.arm(kind, buffer) }?;

        let submit_result = submit(self.raw(), self.context_value());

        if let Err(error) = submit_result {
            // SAFETY: the context remains valid while the guard is alive. The
            // token atomically wins only if no inline callback already consumed
            // the operation, so synchronous failure cannot double-complete it.
            if let Some(active) = unsafe { self.context.as_ref() }.take_token(token) {
                active.completion.send(CompletionResult::error(error, None, active.buffer));
            }
        }

        Ok(OperationFuture {
            receiver,
            _guard: PhantomData,
        })
    }
}

// SAFETY: moving the guard transfers the sole request-handle close authority.
// The context pointer remains valid independently through HANDLE_CLOSING.
unsafe impl Send for RequestGuard {}

impl Drop for RequestGuard {
    fn drop(&mut self) {
        drop(self.request.take());
    }
}

#[derive(Debug)]
pub(crate) struct OperationFuture<'guard> {
    receiver: RawReceiver<CompletionResult>,
    _guard: PhantomData<&'guard mut RequestGuard>,
}

impl Future for OperationFuture<'_> {
    type Output = std::result::Result<CompletionResult, Disconnected>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // SAFETY: projection does not move the pinned receiver.
        unsafe { self.map_unchecked_mut(|this| &mut this.receiver) }.poll(cx)
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::c_void;
    use std::ptr::NonNull;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;

    use bytesbuf::BytesView;
    use bytesbuf::mem::GlobalPool;
    use plurality::Pool;
    use static_assertions::{assert_impl_all, assert_not_impl_any};
    use windows::Win32::Networking::WinHttp::{
        WINHTTP_ASYNC_RESULT, WINHTTP_CALLBACK_STATUS_CONNECTED_TO_SERVER, WINHTTP_CALLBACK_STATUS_CONNECTING_TO_SERVER,
        WINHTTP_CALLBACK_STATUS_DATA_AVAILABLE, WINHTTP_CALLBACK_STATUS_HANDLE_CLOSING, WINHTTP_CALLBACK_STATUS_HANDLE_CREATED,
        WINHTTP_CALLBACK_STATUS_HEADERS_AVAILABLE, WINHTTP_CALLBACK_STATUS_READ_COMPLETE, WINHTTP_CALLBACK_STATUS_REQUEST_ERROR,
        WINHTTP_CALLBACK_STATUS_SECURE_FAILURE, WINHTTP_CALLBACK_STATUS_SENDREQUEST_COMPLETE, WINHTTP_CALLBACK_STATUS_WRITE_COMPLETE,
    };

    use super::{ContextPool, OperationFuture, RequestGuard, RequestSetup};
    use crate::bindings::{Facade, MockBindings};
    use crate::callback::dispatch_completion;
    use crate::context::{ColdConnectState, CompletionResult, OperationBuffer, OperationKind};
    use crate::error::{WinHttpError, WinHttpOperation};
    use crate::handle::{ConnectHandle, RawHandle, RequestHandle, SessionHandle};
    use crate::options::WINHTTP_OPTION_CONTEXT_VALUE;
    use crate::session::WinHttpSession;

    assert_impl_all!(ContextPool: Send, Sync, std::fmt::Debug);
    assert_impl_all!(RequestSetup: Send, std::fmt::Debug);
    assert_impl_all!(RequestGuard: Send, std::fmt::Debug);
    assert_not_impl_any!(RequestGuard: Sync);
    assert_impl_all!(OperationFuture<'static>: Send, std::fmt::Debug);
    assert_not_impl_any!(OperationFuture<'static>: Sync);

    const SESSION: usize = 1;
    const CONNECT: usize = 2;
    const REQUEST: usize = 3;

    #[test]
    fn context_option_failure_returns_context_and_closes_each_handle_once() {
        let (facade, closes) = bindings(true);
        let contexts = ContextPool::new(Pool::new());
        let session = session(facade.clone());
        let setup = RequestSetup::new(
            RequestHandle::new(raw_handle(REQUEST), facade.clone()),
            ConnectHandle::new(raw_handle(CONNECT), facade),
            Arc::clone(&session),
            &contexts,
        );

        assert_eq!(contexts.lock().expect("test lock is not poisoned").len(), 1);
        let error = setup.install().expect_err("the context option is configured to fail");

        assert_eq!(error.code(), 12019);
        assert_eq!(contexts.lock().expect("test lock is not poisoned").len(), 0);
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
        let guard = RequestSetup::new(
            RequestHandle::new(raw_handle(REQUEST), facade.clone()),
            ConnectHandle::new(raw_handle(CONNECT), facade),
            Arc::clone(&session),
            &contexts,
        )
        .install()
        .expect("context installation succeeds");
        let context = guard.context_ptr();

        drop(session);
        drop(guard);

        assert_eq!(closes.request.load(Ordering::SeqCst), 1);
        assert_eq!(closes.connect.load(Ordering::SeqCst), 0);
        assert_eq!(closes.session.load(Ordering::SeqCst), 0);
        assert_eq!(contexts.lock().expect("test lock is not poisoned").len(), 1);

        closing(context);

        assert_eq!(contexts.lock().expect("test lock is not poisoned").len(), 0);
        assert_eq!(closes.connect.load(Ordering::SeqCst), 1);
        assert_eq!(closes.session.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn inline_completion_before_submit_returns_is_observed() {
        let (mut guard, context, contexts, session, closes) = installed();
        let future = guard
            .submit(OperationKind::SendRequest, OperationBuffer::none(), |_, context_value| {
                assert_eq!(context_value, context.expose_provenance());
                assert_eq!(context_value, closes.context.load(Ordering::SeqCst));
                complete(context, WINHTTP_CALLBACK_STATUS_SENDREQUEST_COMPLETE, std::ptr::null_mut(), 0);
                Ok(())
            })
            .expect("no operation is already active");

        assert!(matches!(
            futures::executor::block_on(future).expect("sender remains connected"),
            CompletionResult::SendRequestComplete
        ));
        finish(guard, context, &contexts, session, &closes);
    }

    #[test]
    fn foreign_thread_completion_wakes_send_future() {
        let (mut guard, context, contexts, session, closes) = installed();
        let future = guard
            .submit(OperationKind::HeadersAvailable, OperationBuffer::none(), |_, context_value| {
                thread::spawn(move || {
                    let context = std::ptr::with_exposed_provenance_mut(context_value);
                    complete(context, WINHTTP_CALLBACK_STATUS_HEADERS_AVAILABLE, std::ptr::null_mut(), 0);
                });
                Ok(())
            })
            .expect("no operation is already active");

        assert!(matches!(
            futures::executor::block_on(future).expect("sender remains connected"),
            CompletionResult::HeadersAvailable
        ));
        finish(guard, context, &contexts, session, &closes);
    }

    #[test]
    fn synchronous_submit_failure_completes_once() {
        let (mut guard, context, contexts, session, closes) = installed();
        let future = guard
            .submit(OperationKind::SendRequest, OperationBuffer::none(), |_, _| {
                Err(WinHttpError::new(12029, WinHttpOperation::SendRequest))
            })
            .expect("no operation is already active");

        let CompletionResult::Error { error, api_result, buffer } = futures::executor::block_on(future).expect("sender remains connected")
        else {
            panic!("synchronous failure must produce an error completion");
        };
        assert_eq!(error.code(), 12029);
        assert_eq!(api_result, None);
        assert!(buffer.is_none());
        complete(context, WINHTTP_CALLBACK_STATUS_SENDREQUEST_COMPLETE, std::ptr::null_mut(), 0);
        finish(guard, context, &contexts, session, &closes);
    }

    #[test]
    fn sequential_success_statuses_decode_and_return_buffers() {
        let (mut guard, context, contexts, session, closes) = installed();

        let send = guard
            .submit(OperationKind::SendRequest, OperationBuffer::none(), |_, _| {
                complete(context, WINHTTP_CALLBACK_STATUS_SENDREQUEST_COMPLETE, std::ptr::null_mut(), 0);
                Ok(())
            })
            .expect("send slot is idle");
        assert!(matches!(
            futures::executor::block_on(send).expect("send completion"),
            CompletionResult::SendRequestComplete
        ));

        let headers = guard
            .submit(OperationKind::HeadersAvailable, OperationBuffer::none(), |_, _| {
                complete(context, WINHTTP_CALLBACK_STATUS_HEADERS_AVAILABLE, std::ptr::null_mut(), 0);
                Ok(())
            })
            .expect("headers slot is idle");
        assert!(matches!(
            futures::executor::block_on(headers).expect("headers completion"),
            CompletionResult::HeadersAvailable
        ));

        let mut available = 17_u32;
        let data = guard
            .submit(OperationKind::DataAvailable, OperationBuffer::none(), |_, _| {
                complete(
                    context,
                    WINHTTP_CALLBACK_STATUS_DATA_AVAILABLE,
                    (&raw mut available).cast(),
                    status_info_len::<u32>(),
                );
                Ok(())
            })
            .expect("data slot is idle");
        assert!(matches!(
            futures::executor::block_on(data).expect("data completion"),
            CompletionResult::DataAvailable(17)
        ));

        let mut read_memory = [0_u8; 8];
        let read_address = read_memory.as_mut_ptr().addr();
        let read = guard
            .submit(
                OperationKind::Read,
                OperationBuffer::read(GlobalPool::new().reserve(8), read_address, 8),
                |_, _| {
                    complete(context, WINHTTP_CALLBACK_STATUS_READ_COMPLETE, read_memory.as_mut_ptr().cast(), 5);
                    Ok(())
                },
            )
            .expect("read slot is idle");
        assert!(matches!(
            futures::executor::block_on(read).expect("read completion"),
            CompletionResult::ReadComplete { len: 5, .. }
        ));

        let mut written = 4_u32;
        let write = guard
            .submit(
                OperationKind::Write,
                OperationBuffer::write(
                    BytesView::copied_from_slice(b"data", &GlobalPool::new()),
                    NonNull::<u8>::dangling().as_ptr().addr(),
                    4,
                ),
                |_, _| {
                    complete(
                        context,
                        WINHTTP_CALLBACK_STATUS_WRITE_COMPLETE,
                        (&raw mut written).cast(),
                        status_info_len::<u32>(),
                    );
                    Ok(())
                },
            )
            .expect("write slot is idle");
        assert!(matches!(
            futures::executor::block_on(write).expect("write completion"),
            CompletionResult::WriteComplete { len: 4, .. }
        ));
        finish(guard, context, &contexts, session, &closes);
    }

    #[test]
    fn cancellation_retains_read_and_write_operations_until_handle_closing() {
        for buffer in [
            OperationBuffer::read(GlobalPool::new().reserve(8), NonNull::<u8>::dangling().as_ptr().addr(), 8),
            OperationBuffer::write(
                BytesView::copied_from_slice(b"outstanding", &GlobalPool::new()),
                NonNull::<u8>::dangling().as_ptr().addr(),
                11,
            ),
        ] {
            let (mut guard, context, contexts, session, closes) = installed();
            let kind = match buffer {
                OperationBuffer::Read { .. } => OperationKind::Read,
                OperationBuffer::Write { .. } => OperationKind::Write,
                OperationBuffer::None => unreachable!("test buffers are read or write"),
            };
            let future = guard.submit(kind, buffer, |_, _| Ok(())).expect("no operation is already active");

            assert_eq!(contexts.lock().expect("test lock is not poisoned").len(), 1);
            drop(future);
            drop(session);
            drop(guard);

            assert_eq!(closes.request.load(Ordering::SeqCst), 1);
            assert_eq!(closes.connect.load(Ordering::SeqCst), 0);
            assert_eq!(closes.session.load(Ordering::SeqCst), 0);
            assert_eq!(contexts.lock().expect("test lock is not poisoned").len(), 1);

            closing(context);

            assert_eq!(contexts.lock().expect("test lock is not poisoned").len(), 0);
            assert_eq!(closes.connect.load(Ordering::SeqCst), 1);
            assert_eq!(closes.session.load(Ordering::SeqCst), 1);
        }
    }

    #[test]
    fn request_error_classification_uses_error_code_in_both_secure_status_orders() {
        for secure_first in [true, false] {
            let (mut guard, context, contexts, session, closes) = installed();
            let future = guard
                .submit(OperationKind::SendRequest, OperationBuffer::none(), |_, _| Ok(()))
                .expect("no operation is already active");
            let mut secure_flags = 0x20_u32;
            let mut async_result = WINHTTP_ASYNC_RESULT {
                dwResult: 7,
                dwError: 12175,
            };

            if secure_first {
                complete(
                    context,
                    WINHTTP_CALLBACK_STATUS_SECURE_FAILURE,
                    (&raw mut secure_flags).cast(),
                    status_info_len::<u32>(),
                );
            }
            complete(
                context,
                WINHTTP_CALLBACK_STATUS_REQUEST_ERROR,
                (&raw mut async_result).cast(),
                status_info_len::<WINHTTP_ASYNC_RESULT>(),
            );

            let CompletionResult::Error { error, api_result, buffer } = futures::executor::block_on(future).expect("error completion")
            else {
                panic!("request error must produce an error completion");
            };
            assert_eq!(error.code(), 12175);
            assert_eq!(api_result, Some(7));
            assert!(buffer.is_none());
            assert_eq!(error.secure_failure_flags(), secure_first.then_some(0x20));

            if !secure_first {
                complete(
                    context,
                    WINHTTP_CALLBACK_STATUS_SECURE_FAILURE,
                    (&raw mut secure_flags).cast(),
                    status_info_len::<u32>(),
                );
                // SAFETY: the guard is still alive, so callback ownership keeps
                // the installed context valid.
                assert_eq!(unsafe { &*context }.secure_failure_flags(), Some(0x20));
            }

            finish(guard, context, &contexts, session, &closes);
        }
    }

    #[test]
    fn duplicate_and_late_completions_cannot_send_twice() {
        let (mut guard, context, contexts, session, closes) = installed();
        let future = guard
            .submit(OperationKind::SendRequest, OperationBuffer::none(), |_, _| Ok(()))
            .expect("no operation is already active");

        complete(context, WINHTTP_CALLBACK_STATUS_SENDREQUEST_COMPLETE, std::ptr::null_mut(), 0);
        complete(context, WINHTTP_CALLBACK_STATUS_SENDREQUEST_COMPLETE, std::ptr::null_mut(), 0);
        assert!(matches!(
            futures::executor::block_on(future).expect("first completion"),
            CompletionResult::SendRequestComplete
        ));
        complete(context, WINHTTP_CALLBACK_STATUS_SENDREQUEST_COMPLETE, std::ptr::null_mut(), 0);

        finish(guard, context, &contexts, session, &closes);
    }

    #[test]
    fn malformed_status_info_is_not_dereferenced() {
        let (mut guard, context, contexts, session, closes) = installed();
        let future = guard
            .submit(OperationKind::SendRequest, OperationBuffer::none(), |_, _| Ok(()))
            .expect("no operation is already active");
        let mut bytes = [0_u8; size_of::<WINHTTP_ASYNC_RESULT>() + align_of::<WINHTTP_ASYNC_RESULT>()];
        let offset = (0..align_of::<WINHTTP_ASYNC_RESULT>())
            .find(|offset| !(bytes.as_ptr().addr() + offset).is_multiple_of(align_of::<WINHTTP_ASYNC_RESULT>()))
            .expect("WINHTTP_ASYNC_RESULT has alignment greater than one");
        // SAFETY: offset is less than the type alignment, and the byte array has
        // that much padding beyond the status structure's required length.
        let unaligned = unsafe { bytes.as_mut_ptr().add(offset) }.cast();

        complete(
            context,
            WINHTTP_CALLBACK_STATUS_REQUEST_ERROR,
            unaligned,
            status_info_len::<WINHTTP_ASYNC_RESULT>(),
        );

        assert!(matches!(
            futures::executor::block_on(future).expect("invalid-info completion"),
            CompletionResult::InvalidStatusInfo {
                status: WINHTTP_CALLBACK_STATUS_REQUEST_ERROR,
                ..
            }
        ));

        finish(guard, context, &contexts, session, &closes);
    }

    #[test]
    fn connect_attribution_is_bounded_and_handle_created_is_inert() {
        let (guard, context, contexts, session, closes) = installed();

        complete(context, WINHTTP_CALLBACK_STATUS_HANDLE_CREATED, std::ptr::null_mut(), 0);
        // SAFETY: the guard is alive and therefore the context is valid.
        assert_eq!(unsafe { &*context }.cold_connect_state(), ColdConnectState::Unobserved);

        complete(context, WINHTTP_CALLBACK_STATUS_CONNECTING_TO_SERVER, std::ptr::null_mut(), 0);
        // SAFETY: the guard is alive and therefore the context is valid.
        assert_eq!(unsafe { &*context }.cold_connect_state(), ColdConnectState::Connecting);

        complete(context, WINHTTP_CALLBACK_STATUS_CONNECTED_TO_SERVER, std::ptr::null_mut(), 0);
        // SAFETY: the guard is alive and therefore the context is valid.
        assert_eq!(unsafe { &*context }.cold_connect_state(), ColdConnectState::Connected);

        finish(guard, context, &contexts, session, &closes);
    }

    #[test]
    fn installed_context_outlives_its_pool_handle() {
        let (facade, closes) = bindings(false);
        let contexts = ContextPool::new(Pool::new());
        let session = session(facade.clone());
        let guard = RequestSetup::new(
            RequestHandle::new(raw_handle(REQUEST), facade.clone()),
            ConnectHandle::new(raw_handle(CONNECT), facade),
            Arc::clone(&session),
            &contexts,
        )
        .install()
        .expect("context installation succeeds");
        let context = guard.context_ptr();

        drop(contexts);

        drop(session);
        drop(guard);
        closing(context);
        assert_eq!(closes.request.load(Ordering::SeqCst), 1);
        assert_eq!(closes.connect.load(Ordering::SeqCst), 1);
        assert_eq!(closes.session.load(Ordering::SeqCst), 1);
    }

    fn installed() -> (
        RequestGuard,
        *mut crate::context::RequestContext,
        ContextPool,
        Arc<WinHttpSession>,
        Arc<CloseCounts>,
    ) {
        let (facade, closes) = bindings(false);
        let contexts = ContextPool::new(Pool::new());
        let session = session(facade.clone());
        let guard = RequestSetup::new(
            RequestHandle::new(raw_handle(REQUEST), facade.clone()),
            ConnectHandle::new(raw_handle(CONNECT), facade),
            Arc::clone(&session),
            &contexts,
        )
        .install()
        .expect("context installation succeeds");
        let context = guard.context_ptr();

        (guard, context, contexts, session, closes)
    }

    fn finish(
        guard: RequestGuard,
        context: *mut crate::context::RequestContext,
        contexts: &ContextPool,
        session: Arc<WinHttpSession>,
        closes: &CloseCounts,
    ) {
        drop(session);
        drop(guard);
        closing(context);

        assert_eq!(contexts.lock().expect("test lock is not poisoned").len(), 0);
        assert_eq!(closes.request.load(Ordering::SeqCst), 1);
        assert_eq!(closes.connect.load(Ordering::SeqCst), 1);
        assert_eq!(closes.session.load(Ordering::SeqCst), 1);
    }

    fn complete(context: *mut crate::context::RequestContext, status: u32, info: *mut c_void, len: u32) {
        // SAFETY: every test passes a live installed context and preserves each
        // status-info object for the duration of the synchronous dispatch.
        unsafe {
            dispatch_completion(context, status, info, len);
        }
    }

    fn status_info_len<T>() -> u32 {
        u32::try_from(size_of::<T>()).expect("WinHTTP status-info types fit in a DWORD length")
    }

    fn closing(context: *mut crate::context::RequestContext) {
        complete(context, WINHTTP_CALLBACK_STATUS_HANDLE_CLOSING, std::ptr::null_mut(), 0);
    }

    fn session(facade: Facade) -> Arc<WinHttpSession> {
        Arc::new(WinHttpSession::from_handle(SessionHandle::new(raw_handle(SESSION), facade)))
    }

    fn bindings(fail_context_option: bool) -> (Facade, Arc<CloseCounts>) {
        let closes = Arc::new(CloseCounts::default());
        let mut bindings = MockBindings::new();
        let context_counts = Arc::clone(&closes);
        bindings
            .expect_set_option()
            .withf(|handle, option, value| {
                *handle == raw_handle(REQUEST)
                    && *option == WINHTTP_OPTION_CONTEXT_VALUE
                    && value.len() == size_of::<usize>()
                    && usize::from_ne_bytes(value.try_into().expect("the context option is exactly pointer-sized")) != 0
            })
            .once()
            .returning(move |_, _, value| {
                context_counts.context.store(
                    usize::from_ne_bytes(value.try_into().expect("the context option is exactly pointer-sized")),
                    Ordering::SeqCst,
                );
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

        (Facade::mock(Arc::new(bindings)), closes)
    }

    #[derive(Default)]
    struct CloseCounts {
        context: AtomicUsize,
        session: AtomicUsize,
        connect: AtomicUsize,
        request: AtomicUsize,
    }

    fn raw_handle(value: usize) -> RawHandle {
        RawHandle::new(std::ptr::without_provenance_mut::<c_void>(value)).expect("test handle values are nonzero")
    }
}
