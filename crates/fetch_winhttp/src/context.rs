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

// No ActiveOperation is initialized in the operation slot.
const OPERATION_IDLE: u8 = 0;
// One callback or the submitting thread has claimed the initialized payload.
const OPERATION_CLAIMED: u8 = 1;
// The low 32 bits store WinHTTP's secure-failure flags. This bit distinguishes
// "no callback observed" from a callback that reported a zero flag mask.
const SECURE_FAILURE_PRESENT: u64 = 1 << u32::BITS;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
/// Tags the callback payload stored for one asynchronous WinHTTP call.
///
/// The tag lets a completion callback validate its status before claiming the
/// sender and retained buffer. It also supplies the operation context used when
/// a `REQUEST_ERROR` is converted into a [`WinHttpError`].
///
/// Values start at 2 because 0 and 1 are reserved for the operation slot's idle
/// and claimed states; they do not correspond to WinHTTP constants.
pub(crate) enum OperationKind {
    SendRequest = 2,
    HeadersAvailable,
    DataAvailable,
    Read,
    Write,
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
    reason = "boxing is unacceptable on hot paths, so buffers require inline storage"
)]
/// Keeps asynchronous I/O storage alive until WinHTTP releases it.
///
/// Read and write calls lend raw pointers into these buffers to WinHTTP. Moving
/// the owning buffer into this enum prevents the request task from freeing or
/// mutating that storage before a completion callback returns ownership. A read
/// additionally records the exposed address and capacity so the callback can
/// reject metadata that would claim initialization outside the lent span. A
/// write records the exact submitted length while retaining the immutable view.
///
/// An operation owns at most one buffer because the request lifecycle never has
/// more than one asynchronous WinHTTP call outstanding.
pub(crate) enum OperationBuffer {
    None,
    Read { buffer: BytesBuf, address: usize, capacity: u32 },
    Write { buffer: BytesView, len: u32 },
}

impl OperationBuffer {
    pub(crate) const fn none() -> Self {
        Self::None
    }

    pub(crate) fn read(buffer: BytesBuf, address: usize, capacity: u32) -> Self {
        Self::Read { buffer, address, capacity }
    }

    pub(crate) fn write(buffer: BytesView, len: u32) -> Self {
        Self::Write { buffer, len }
    }

    pub(crate) fn into_completion(self) -> Option<CompletionBuffer> {
        match self {
            Self::None => None,
            Self::Read { buffer, .. } => Some(CompletionBuffer::Read { _buffer: buffer }),
            Self::Write { buffer, .. } => Some(CompletionBuffer::Write { _buffer: buffer }),
        }
    }
}

#[derive(Debug)]
#[expect(
    clippy::large_enum_variant,
    reason = "boxing is unacceptable on hot paths, so buffers require inline storage"
)]
/// Retains an I/O buffer when a completion cannot return it directly.
///
/// Successful reads and writes return their buffer in the corresponding result
/// variant. Error-shaped results keep it here only so the buffer is dropped
/// after WinHTTP has completed or abandoned the operation.
pub(crate) enum CompletionBuffer {
    Read { _buffer: BytesBuf },
    Write { _buffer: BytesView },
}

#[derive(Debug)]
/// Transfers one decoded callback outcome back to the request task.
///
/// The callback constructs this payload only after claiming the operation
/// slot. Sending it through the embedded event transfers retained buffer
/// ownership back to the future and wakes the executor without blocking a
/// WinHTTP callback thread. Length-bearing variants are emitted only after the
/// callback metadata has been checked against the operation and retained
/// buffer; malformed metadata is represented separately from an operating
/// system error.
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
        _buffer: Option<CompletionBuffer>,
    },
    InvalidStatusInfo {
        status: u32,
        len: u32,
        _buffer: Option<CompletionBuffer>,
    },
}

impl CompletionResult {
    pub(crate) fn error(error: WinHttpError, buffer: OperationBuffer) -> Self {
        Self::Error {
            error,
            _buffer: buffer.into_completion(),
        }
    }

    pub(crate) fn invalid_status_info(status: u32, len: u32, buffer: OperationBuffer) -> Self {
        Self::InvalidStatusInfo {
            status,
            len,
            _buffer: buffer.into_completion(),
        }
    }
}

/// Bundles the resources transferred from the request task to one callback.
///
/// The operation slot publishes this value after it is fully initialized. The
/// completion callback, synchronous submission-failure path, or final-close
/// path that atomically claims the slot becomes the sole owner of the event
/// sender and any buffer lent to WinHTTP. The kind, sender, and buffer therefore
/// move as one unit and cannot be paired with different operations.
pub(crate) struct ActiveOperation {
    pub(crate) kind: OperationKind,
    pub(crate) completion: RawSender<CompletionResult>,
    pub(crate) buffer: OperationBuffer,
}

/// Stores the callback handoff payload without allocating per operation.
///
/// Sequential submission is guaranteed by the mutable borrow held by
/// `OperationFuture`; this type does not enforce that API rule in release
/// builds. Its atomic tag instead publishes the initialized payload to WinHTTP
/// threads and arbitrates the real race between a completion callback, a
/// synchronous submission failure, and final handle closure. The operation
/// future owns the request handle until its receiver endpoint is destroyed, so
/// cancellation cannot expose a reusable guard while this payload remains live.
struct CallbackOperationSlot {
    state: AtomicU8,
    active: UnsafeCell<MaybeUninit<ActiveOperation>>,
    completion: UnsafeCell<EmbeddedEvent<CompletionResult>>,
}

impl CallbackOperationSlot {
    fn new() -> Self {
        Self {
            state: AtomicU8::new(OPERATION_IDLE),
            active: UnsafeCell::new(MaybeUninit::uninit()),
            completion: UnsafeCell::new(EmbeddedEvent::new()),
        }
    }

    unsafe fn arm(self: Pin<&Self>, kind: OperationKind, buffer: OperationBuffer) -> RawReceiver<CompletionResult> {
        debug_assert_eq!(
            self.state.load(Ordering::Acquire),
            OPERATION_IDLE,
            "OperationFuture's mutable RequestGuard borrow prevents overlapping operations"
        );

        let completion = self.completion.get();
        // SAFETY: the caller guarantees that no operation is already armed and
        // that this slot remains pinned until both event endpoints are gone.
        let completion = unsafe { &mut *completion };
        // SAFETY: the operation containing this storage is pinned for the
        // endpoints' full lifetimes.
        let completion = unsafe { Pin::new_unchecked(completion) };
        // SAFETY: the pinned slot outlives both endpoints, and the caller's
        // exclusive RequestGuard borrow guarantees the storage is not in use.
        let (completion, receiver) = unsafe { Event::placed(completion) };

        // SAFETY: sequential submission gives this caller exclusive access to
        // the uninitialized slot. The value is published by the release store
        // below only after initialization finishes.
        unsafe {
            (*self.active.get()).write(ActiveOperation { kind, completion, buffer });
        }
        self.state.store(kind as u8, Ordering::Release);

        receiver
    }

    fn take_for_status(&self, status: u32) -> Option<ActiveOperation> {
        let tag = self.state.load(Ordering::Acquire);
        let kind = active_kind(tag)?;

        if kind.callback_status() != status {
            return None;
        }

        self.take_kind(kind)
    }

    fn take_kind(&self, kind: OperationKind) -> Option<ActiveOperation> {
        self.state
            .compare_exchange(kind as u8, OPERATION_CLAIMED, Ordering::AcqRel, Ordering::Acquire)
            .ok()?;

        let active = self.active.get();
        // SAFETY: the exact published tag was atomically claimed above, so
        // this callback has exclusive access to the initialized storage.
        let active = unsafe { &*active };
        // SAFETY: arming initialized the value before publishing the tag, and
        // the successful claim grants this callback sole ownership.
        let active = unsafe { active.assume_init_read() };
        self.state.store(OPERATION_IDLE, Ordering::Release);

        Some(active)
    }

    fn take_any(&self) -> Option<ActiveOperation> {
        let kind = active_kind(self.state.load(Ordering::Acquire))?;

        self.take_kind(kind)
    }

    fn is_idle(&self) -> bool {
        self.state.load(Ordering::Acquire) == OPERATION_IDLE
    }
}

impl Drop for CallbackOperationSlot {
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
                "HANDLE_CLOSING must not overlap the callback that claimed the operation"
            );
        }
    }
}

fn active_kind(state: u8) -> Option<OperationKind> {
    if state <= OPERATION_CLAIMED {
        return None;
    }

    OperationKind::from_discriminant(state)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
/// Summarizes connection-establishment notifications for request telemetry.
///
/// The callback records these states independently of operation completions so
/// a later failure can be attributed to a cold connection attempt. The numeric
/// values are stored directly in `RequestContext::cold_connect`.
///
/// `Unobserved` means no connection-establishment callback was seen.
/// `Connecting` and `Connected` both identify work associated with a newly
/// established connection and permit elapsed time to be attached to rich error
/// telemetry. The state does not affect retry classification or metric labels.
pub(crate) enum ColdConnectState {
    Unobserved = 0,
    Connecting = 1,
    Connected = 2,
}

/// Owns callback-visible state and parent handles for one request.
///
/// A pooled instance is pinned, installed as the WinHTTP request context, and
/// reclaimed only by the final `HANDLE_CLOSING` callback. It provides the
/// callback-to-future operation slot, records request-scoped diagnostics, and
/// keeps the connect and session handles alive while WinHTTP may still access
/// their child request.
///
/// Installing the pointer transfers reclamation authority from the request
/// task to WinHTTP. The stable address is required both by the WinHTTP context
/// value and by the embedded one-shot event storage. Concurrent callbacks use
/// only shared references: mutable callback state is atomic or protected by the
/// operation slot's atomic ownership transfer. Final reclamation returns the
/// allocation to its `plurality` pool after its parent handles are released.
pub(crate) struct RequestContext {
    // Publishes the current event sender and retained I/O buffer to callbacks.
    operation: CallbackOperationSlot,
    // Low 32 bits are WinHTTP secure-failure flags; bit 32 marks their presence.
    secure_failure: AtomicU64,
    // Stores a ColdConnectState discriminant for later telemetry attribution.
    cold_connect: AtomicU8,
    // Keeps the request's parent connect handle alive through HANDLE_CLOSING.
    connect: ConnectHandle,
    // Keeps the owning session and its connection pool alive with the request.
    session: Arc<WinHttpSession>,
    _pinned: PhantomPinned,
}

impl RequestContext {
    pub(crate) fn new(connect: ConnectHandle, session: Arc<WinHttpSession>) -> Self {
        Self {
            operation: CallbackOperationSlot::new(),
            secure_failure: AtomicU64::new(0),
            cold_connect: AtomicU8::new(ColdConnectState::Unobserved as u8),
            connect,
            session,
            _pinned: PhantomPinned,
        }
    }

    pub(crate) unsafe fn arm(self: Pin<&Self>, kind: OperationKind, buffer: OperationBuffer) -> RawReceiver<CompletionResult> {
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

    pub(crate) fn take_kind(&self, kind: OperationKind) -> Option<ActiveOperation> {
        self.operation.take_kind(kind)
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
// protected by CallbackOperationSlot's atomic ownership transfer. The connect
// handle and session owner are never exposed or accessed until exclusive final
// destruction after HANDLE_CLOSING.
unsafe impl Sync for RequestContext {}

#[cfg(test)]
mod tests {
    use std::panic::{RefUnwindSafe, UnwindSafe};

    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::{
        ActiveOperation, CallbackOperationSlot, ColdConnectState, CompletionBuffer, CompletionResult, OperationBuffer, OperationKind,
        RequestContext,
    };

    assert_impl_all!(OperationKind: UnwindSafe, RefUnwindSafe);
    assert_impl_all!(OperationBuffer: UnwindSafe, RefUnwindSafe);
    assert_impl_all!(CompletionBuffer: UnwindSafe, RefUnwindSafe);
    assert_impl_all!(CompletionResult: Send, std::fmt::Debug, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(ActiveOperation: UnwindSafe, RefUnwindSafe);
    assert_impl_all!(CallbackOperationSlot: UnwindSafe);
    // The operation slot uses UnsafeCell for callback-owned state.
    assert_not_impl_any!(CallbackOperationSlot: RefUnwindSafe);
    assert_impl_all!(ColdConnectState: UnwindSafe, RefUnwindSafe);
    assert_impl_all!(RequestContext: Send, Sync, std::fmt::Debug, UnwindSafe);
    // The context contains the operation slot's actual UnsafeCell state.
    assert_not_impl_any!(RequestContext: RefUnwindSafe);
}
