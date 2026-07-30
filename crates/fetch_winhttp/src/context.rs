// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::cell::UnsafeCell;
use std::fmt;
use std::marker::PhantomPinned;
use std::mem::MaybeUninit;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};

use bytesbuf::{BytesBuf, BytesView};
use events_once::{EmbeddedEvent, Event, RawReceiver, RawSender};
use windows::Win32::Networking::WinHttp::{
    WINHTTP_CALLBACK_STATUS_DATA_AVAILABLE, WINHTTP_CALLBACK_STATUS_HEADERS_AVAILABLE, WINHTTP_CALLBACK_STATUS_READ_COMPLETE,
    WINHTTP_CALLBACK_STATUS_SENDREQUEST_COMPLETE, WINHTTP_CALLBACK_STATUS_WRITE_COMPLETE,
};

use crate::error::{WinHttpError, WinHttpOperation};
use crate::handle::ConnectHandle;
use crate::session::WinHttpSession;

const OPERATION_IDLE: u64 = 0;
const OPERATION_TRANSITION: u64 = 1;
const OPERATION_KIND_BITS: u32 = 8;
const SECURE_FAILURE_PRESENT: u64 = 1 << u32::BITS;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub(crate) enum OperationKind {
    SendRequest = 2,
    HeadersAvailable = 3,
    DataAvailable = 4,
    Read = 5,
    Write = 6,
}

impl OperationKind {
    pub(crate) const fn callback_status(self) -> u32 {
        match self {
            Self::SendRequest => WINHTTP_CALLBACK_STATUS_SENDREQUEST_COMPLETE,
            Self::HeadersAvailable => WINHTTP_CALLBACK_STATUS_HEADERS_AVAILABLE,
            Self::DataAvailable => WINHTTP_CALLBACK_STATUS_DATA_AVAILABLE,
            Self::Read => WINHTTP_CALLBACK_STATUS_READ_COMPLETE,
            Self::Write => WINHTTP_CALLBACK_STATUS_WRITE_COMPLETE,
        }
    }

    pub(crate) const fn operation(self) -> WinHttpOperation {
        match self {
            Self::SendRequest => WinHttpOperation::SendRequest,
            Self::HeadersAvailable => WinHttpOperation::ReceiveResponse,
            Self::DataAvailable => WinHttpOperation::QueryDataAvailable,
            Self::Read => WinHttpOperation::ReadData,
            Self::Write => WinHttpOperation::WriteData,
        }
    }

    const fn from_discriminant(value: u8) -> Option<Self> {
        match value {
            value if value == Self::SendRequest as u8 => Some(Self::SendRequest),
            value if value == Self::HeadersAvailable as u8 => Some(Self::HeadersAvailable),
            value if value == Self::DataAvailable as u8 => Some(Self::DataAvailable),
            value if value == Self::Read as u8 => Some(Self::Read),
            value if value == Self::Write as u8 => Some(Self::Write),
            _ => None,
        }
    }
}

#[derive(Debug)]
#[expect(
    clippy::large_enum_variant,
    reason = "borrowed buffers stay inline so arming an operation does not allocate"
)]
pub(crate) enum OperationBuffer {
    None,
    Read { buffer: BytesBuf, address: usize, capacity: u32 },
    Write { buffer: BytesView, address: usize, len: u32 },
}

impl OperationBuffer {
    pub(crate) const fn none() -> Self {
        Self::None
    }

    pub(crate) fn read(buffer: BytesBuf, address: usize, capacity: u32) -> Self {
        Self::Read { buffer, address, capacity }
    }

    pub(crate) fn write(buffer: BytesView, address: usize, len: u32) -> Self {
        Self::Write { buffer, address, len }
    }

    pub(crate) fn into_completion(self) -> Option<CompletionBuffer> {
        match self {
            Self::None => None,
            Self::Read { buffer, .. } => Some(CompletionBuffer::Read(buffer)),
            Self::Write { buffer, .. } => Some(CompletionBuffer::Write(buffer)),
        }
    }
}

#[derive(Debug)]
#[expect(
    clippy::large_enum_variant,
    reason = "completed buffers transfer inline through the embedded event without allocation"
)]
pub(crate) enum CompletionBuffer {
    Read(BytesBuf),
    Write(BytesView),
}

#[derive(Debug)]
pub(crate) enum CompletionResult {
    SendRequestComplete,
    HeadersAvailable,
    DataAvailable(u32),
    ReadComplete {
        buffer: BytesBuf,
        len: u32,
    },
    WriteComplete {
        buffer: BytesView,
        len: u32,
    },
    Error {
        error: WinHttpError,
        api_result: Option<usize>,
        buffer: Option<CompletionBuffer>,
    },
    InvalidStatusInfo {
        status: u32,
        len: u32,
        buffer: Option<CompletionBuffer>,
    },
}

impl CompletionResult {
    pub(crate) fn error(error: WinHttpError, api_result: Option<usize>, buffer: OperationBuffer) -> Self {
        Self::Error {
            error,
            api_result,
            buffer: buffer.into_completion(),
        }
    }

    pub(crate) fn invalid_status_info(status: u32, len: u32, buffer: OperationBuffer) -> Self {
        Self::InvalidStatusInfo {
            status,
            len,
            buffer: buffer.into_completion(),
        }
    }
}

pub(crate) struct ActiveOperation {
    pub(crate) kind: OperationKind,
    pub(crate) completion: RawSender<CompletionResult>,
    pub(crate) buffer: OperationBuffer,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct OperationToken(u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct OperationAlreadyActive;

struct SequentialOperation {
    state: AtomicU64,
    generation: AtomicU64,
    active: UnsafeCell<MaybeUninit<ActiveOperation>>,
    completion: UnsafeCell<EmbeddedEvent<CompletionResult>>,
}

impl SequentialOperation {
    fn new() -> Self {
        Self {
            state: AtomicU64::new(OPERATION_IDLE),
            generation: AtomicU64::new(0),
            active: UnsafeCell::new(MaybeUninit::uninit()),
            completion: UnsafeCell::new(EmbeddedEvent::new()),
        }
    }

    unsafe fn arm(
        self: Pin<&Self>,
        kind: OperationKind,
        buffer: OperationBuffer,
    ) -> Result<(OperationToken, RawReceiver<CompletionResult>), OperationAlreadyActive> {
        self.state
            .compare_exchange(OPERATION_IDLE, OPERATION_TRANSITION, Ordering::Acquire, Ordering::Relaxed)
            .map_err(|_current| OperationAlreadyActive)?;

        let completion = self.completion.get();
        // SAFETY: claiming OPERATION_TRANSITION grants exclusive access to the
        // embedded event storage. The caller guarantees that this operation
        // remains pinned until both endpoints are gone.
        let completion = unsafe { &mut *completion };
        // SAFETY: the operation containing this storage is pinned for the
        // endpoints' full lifetimes.
        let completion = unsafe { Pin::new_unchecked(completion) };
        // SAFETY: the pinned operation outlives both endpoints, and the state
        // transition guarantees that the storage is not already in use.
        let (completion, receiver) = unsafe { Event::placed(completion) };

        let generation = self.generation.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
        let token = OperationToken((generation << OPERATION_KIND_BITS) | u64::from(kind as u8));

        // SAFETY: the successful idle-to-transition exchange gives this caller
        // exclusive access to the uninitialized slot. The value is published by
        // the release store below only after initialization finishes.
        unsafe {
            (*self.active.get()).write(ActiveOperation { kind, completion, buffer });
        }
        self.state.store(token.0, Ordering::Release);

        Ok((token, receiver))
    }

    fn take_for_status(&self, status: u32) -> Option<ActiveOperation> {
        let token = self.state.load(Ordering::Acquire);
        let kind = active_kind(token)?;

        if kind.callback_status() != status {
            return None;
        }

        self.take_token(OperationToken(token))
    }

    fn take_token(&self, token: OperationToken) -> Option<ActiveOperation> {
        self.state
            .compare_exchange(token.0, OPERATION_TRANSITION, Ordering::AcqRel, Ordering::Acquire)
            .ok()?;

        let active = self.active.get();
        // SAFETY: the exact published token was atomically claimed above, so
        // this callback has exclusive access to the initialized storage.
        let active = unsafe { &*active };
        // SAFETY: arming initialized the value before publishing the token, and
        // the successful claim grants this callback sole ownership.
        let active = unsafe { active.assume_init_read() };
        self.state.store(OPERATION_IDLE, Ordering::Release);

        Some(active)
    }

    fn take_any(&self) -> Option<ActiveOperation> {
        let token = self.state.load(Ordering::Acquire);
        active_kind(token)?;

        self.take_token(OperationToken(token))
    }

    fn is_idle(&self) -> bool {
        self.state.load(Ordering::Acquire) == OPERATION_IDLE
    }
}

impl Drop for SequentialOperation {
    fn drop(&mut self) {
        let state = *self.state.get_mut();

        if active_kind(state).is_some() {
            // SAFETY: dropping RequestContext requires exclusive ownership. An
            // active value is therefore uniquely owned here and must be dropped
            // so its sender and optional buffer are returned exactly once.
            unsafe {
                self.active.get_mut().assume_init_drop();
            }
        } else {
            debug_assert_eq!(
                state, OPERATION_IDLE,
                "RequestContext dropped while an operation transition was in progress"
            );
        }
    }
}

fn active_kind(token: u64) -> Option<OperationKind> {
    if token <= OPERATION_TRANSITION {
        return None;
    }

    let kind = u8::try_from(token & u64::from(u8::MAX)).expect("masking to the low eight bits guarantees a u8 value");

    OperationKind::from_discriminant(kind)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub(crate) enum ColdConnectState {
    Unobserved = 0,
    Connecting = 1,
    Connected = 2,
}

pub(crate) struct RequestContext {
    operation: SequentialOperation,
    secure_failure: AtomicU64,
    cold_connect: AtomicU8,
    connect: ConnectHandle,
    session: Arc<WinHttpSession>,
    _pinned: PhantomPinned,
}

impl RequestContext {
    pub(crate) fn new(connect: ConnectHandle, session: Arc<WinHttpSession>) -> Self {
        Self {
            operation: SequentialOperation::new(),
            secure_failure: AtomicU64::new(0),
            cold_connect: AtomicU8::new(ColdConnectState::Unobserved as u8),
            connect,
            session,
            _pinned: PhantomPinned,
        }
    }

    pub(crate) unsafe fn arm(
        self: Pin<&Self>,
        kind: OperationKind,
        buffer: OperationBuffer,
    ) -> Result<(OperationToken, RawReceiver<CompletionResult>), OperationAlreadyActive> {
        // SAFETY: RequestContext structurally pins its operation field, and the
        // caller guarantees that the context remains pinned through both event
        // endpoints' lifetimes.
        let operation = unsafe { self.map_unchecked(|context| &context.operation) };

        // SAFETY: forwarded from this method's caller.
        unsafe { operation.arm(kind, buffer) }
    }

    pub(crate) fn take_for_status(&self, status: u32) -> Option<ActiveOperation> {
        self.operation.take_for_status(status)
    }

    pub(crate) fn take_token(&self, token: OperationToken) -> Option<ActiveOperation> {
        self.operation.take_token(token)
    }

    pub(crate) fn take_any(&self) -> Option<ActiveOperation> {
        self.operation.take_any()
    }

    pub(crate) fn is_idle(&self) -> bool {
        self.operation.is_idle()
    }

    pub(crate) fn record_secure_failure(&self, flags: u32) {
        self.secure_failure
            .store(SECURE_FAILURE_PRESENT | u64::from(flags), Ordering::Release);
    }

    pub(crate) fn secure_failure_flags(&self) -> Option<u32> {
        let value = self.secure_failure.load(Ordering::Acquire);

        if value & SECURE_FAILURE_PRESENT == 0 {
            None
        } else {
            Some(u32::try_from(value & u64::from(u32::MAX)).expect("masking to the low 32 bits guarantees a u32 value"))
        }
    }

    pub(crate) fn mark_connecting(&self) {
        self.cold_connect.store(ColdConnectState::Connecting as u8, Ordering::Release);
    }

    pub(crate) fn mark_connected(&self) {
        self.cold_connect.store(ColdConnectState::Connected as u8, Ordering::Release);
    }

    pub(crate) fn cold_connect_state(&self) -> ColdConnectState {
        match self.cold_connect.load(Ordering::Acquire) {
            value if value == ColdConnectState::Connecting as u8 => ColdConnectState::Connecting,
            value if value == ColdConnectState::Connected as u8 => ColdConnectState::Connected,
            _ => ColdConnectState::Unobserved,
        }
    }
}

impl fmt::Debug for RequestContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RequestContext")
            .field("operation_idle", &self.is_idle())
            .field("secure_failure_flags", &self.secure_failure_flags())
            .field("cold_connect_state", &self.cold_connect_state())
            .field("connect", &self.connect)
            .field("session", &self.session)
            .finish_non_exhaustive()
    }
}

// SAFETY: callbacks may share a RequestContext across WinHTTP threads. All
// concurrently accessible fields are atomic, and the operation payload is
// protected by SequentialOperation's atomic ownership transfer. The connect
// handle and session owner are never exposed or accessed until exclusive final
// destruction after HANDLE_CLOSING.
unsafe impl Sync for RequestContext {}

#[cfg(test)]
mod tests {
    use static_assertions::assert_impl_all;

    use super::{CompletionResult, RequestContext};

    assert_impl_all!(CompletionResult: Send, std::fmt::Debug);
    assert_impl_all!(RequestContext: Send, Sync, std::fmt::Debug);
}
