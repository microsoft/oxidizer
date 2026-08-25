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
    /// Every variant, in declaration order.
    ///
    /// Tables derived from this enumeration - such as the notification-mask
    /// coverage test in `session.rs` - iterate this constant instead of
    /// repeating the variant list, so one edit keeps them honest.
    ///
    /// What the const check below enforces: the listed variants occupy the
    /// contiguous discriminant range starting at `SendRequest`, each exactly
    /// once, and no further discriminant decodes through `from_discriminant`.
    /// A variant inserted anywhere but the end, removed, reordered, or
    /// duplicated therefore fails to compile.
    ///
    /// What it cannot enforce: stable Rust cannot count an enumeration's
    /// variants, so a variant appended after `Write` and wired up nowhere else
    /// would leave this list stale. In practice such a variant cannot function
    /// without an arm in `from_discriminant` - without one, `active_kind`
    /// never decodes its slot tag and no callback ever claims its payload -
    /// and adding that arm does fail the check below.
    pub(crate) const ALL: [Self; 5] = [
        Self::SendRequest,
        Self::HeadersAvailable,
        Self::DataAvailable,
        Self::Read,
        Self::Write,
    ];

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

// Keeps `OperationKind::ALL` in sync with the enumeration; see its comment for
// the exact reach and limits of this check. Evaluated eagerly because free
// const items are always const-evaluated.
//
// Stated as flat assertions rather than a loop over the array. A loop here runs
// during const evaluation, and rewriting its induction step - as mutation
// testing does - yields a loop that never terminates, which hangs the compiler
// instead of failing a test. Ref: AGENTS.md, "Code must not hang even under
// mutation testing". Assertion arguments are macro operands, which mutation
// testing leaves alone, so spelling every element out keeps the whole check
// outside that hazard.
const _: () = {
    let first = OperationKind::SendRequest as u8;

    assert!(
        OperationKind::ALL.len() == 5,
        "the element assertion below is written out per element and must grow with OperationKind::ALL"
    );

    assert!(
        OperationKind::ALL[0] as u8 == first
            && OperationKind::ALL[1] as u8 == first + 1
            && OperationKind::ALL[2] as u8 == first + 2
            && OperationKind::ALL[3] as u8 == first + 3
            && OperationKind::ALL[4] as u8 == first + 4,
        "OperationKind::ALL must list every variant exactly once, in declaration order"
    );

    assert!(
        OperationKind::from_discriminant(OperationKind::Write as u8 + 1).is_none(),
        "OperationKind::ALL is missing a variant that from_discriminant decodes"
    );
};

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
/// The slot lives inside the pinned [`RequestContext`] and is reused across the
/// request's sequence of operations: send, each request write, receive, then
/// each response read. It holds the completion sender and any buffer lent to
/// `WinHTTP` for the operation that is currently outstanding, together with the
/// one-shot event whose storage backs that sender.
///
/// Three distinct mechanisms make this sharing between the request task and
/// `WinHTTP` callback threads sound, and each covers a different hazard.
///
/// **Sequential submission is guaranteed by construction, not by this type.**
/// Arming requires the exclusive `RequestGuard` borrow held by
/// `OperationFuture`, and submission moves the request handle out of the guard
/// into that future. Safe code therefore holds no request handle with which to
/// arm a second operation until the future's receiver endpoint is destroyed and
/// the handle returns to the guard. Forgetting the future leaks the handle
/// inside it rather than making the guard reusable. This type only
/// debug-asserts the resulting invariant when arming.
///
/// **The atomic tag publishes the payload and resolves one claim race.** The
/// release store of the operation kind makes the fully initialized payload
/// visible to callback threads, and the compare-exchange in the claim path lets
/// exactly one of the completion callback, the synchronous submission-failure
/// path, and the final-close path move the sender and buffer out. Competing and
/// late claimants observe a failed exchange and do nothing. The tag arbitrates
/// nothing beyond which claimant takes the payload.
///
/// **Correctness additionally requires that `WinHTTP` never delivers
/// `HANDLE_CLOSING` concurrently with another callback for the same request
/// handle.** The tag cannot cover that case and does not try to: claiming the
/// payload is not atomic with reading it out, and the claimed `RawSender`
/// refers to the [`EmbeddedEvent`] stored inline in the context, so the sender
/// still touches context memory after the tag returns to idle. Both the
/// read-out and the send therefore operate on storage that `HANDLE_CLOSING`
/// reclaims, and only genuine non-overlap of the final notification with other
/// callbacks keeps them from racing reclamation.
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

    /// Publishes the payload for one asynchronous `WinHTTP` call and returns
    /// the receiver endpoint that the request task awaits.
    ///
    /// # Safety
    ///
    /// The slot must be idle: no operation may be armed and no claimant may
    /// still hold a payload taken from it. The caller must hold the exclusive
    /// `RequestGuard` borrow that establishes both, because it is the borrow
    /// that keeps a second submission from overwriting live payload and event
    /// storage.
    ///
    /// The event armed by a previous call must have reached its terminal
    /// state, because arming reuses the event storage in place: either its
    /// value was received (the `OperationFuture` polled to `Ready`) or its
    /// receiver was destroyed after the sender had already disconnected. The
    /// previous `RawSender` may still be unwinding on a callback thread.
    /// `events_once` ends an event's lifetime at delivery rather than at
    /// endpoint destruction: `EmbeddedEvent` documents that one container may
    /// back multiple events with non-overlapping lifetimes, and the crate's
    /// own pooled events recycle their storage from the receiving poll while
    /// the sender is still returning from `send()`. Demanding that the sender
    /// object itself be gone would be impossible to meet here, because the
    /// callback thread wakes the request task from inside `RawSender::send()`,
    /// so the woken task can receive the value and re-arm before that call
    /// returns.
    ///
    /// The slot must also remain pinned at a stable address until both
    /// endpoints of the returned event are destroyed, since the sender handed
    /// to `WinHTTP` points into the event stored inline here.
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
        // exclusive RequestGuard borrow guarantees that any previous event
        // reached its terminal state, after which events_once no longer
        // touches this storage.
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

        // The claim and the read-out below are deliberately not one atomic
        // step: the tag stays CLAIMED while the payload moves out and only then
        // returns to idle. Nothing can reclaim the storage inside that window.
        // A competing claimant loses the compare-exchange above. Only closing
        // the request handle can free this storage, by way of the final
        // HANDLE_CLOSING notification, and that close cannot be under way here:
        // a callback claiming the payload runs on a handle whose HANDLE_CLOSING
        // WinHTTP does not deliver concurrently with another callback, and the
        // request task claiming a synchronously failed submission still holds
        // the open request handle it took out of the guard.
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
}

impl fmt::Debug for CallbackOperationSlot {
    #[cfg_attr(coverage_nightly, coverage(off))] // We have no API contract here.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CallbackOperationSlot")
            .field("active", &active_kind(self.state.load(Ordering::Acquire)))
            .finish_non_exhaustive()
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

            return;
        }

        debug_assert_eq!(
            state, OPERATION_IDLE,
            "HANDLE_CLOSING must not overlap the callback that claimed the operation"
        );

        // A claimed tag cannot be observed here. This slot is destroyed only
        // when the context allocation is reclaimed, and that happens either
        // before the context value is installed, when callbacks for the request
        // handle carry a null context and dispatch ignores them, or on the final
        // HANDLE_CLOSING notification, which WinHTTP does not deliver while
        // another callback for the same request handle is executing. A claim
        // therefore always completes, restoring the idle tag, before this
        // destructor can run.
        //
        // If that platform guarantee were violated, the payload would be inside
        // the claimant's read-out window, where this thread cannot tell whether
        // the value has already been moved out. Falling through and leaving the
        // payload untouched does not make that state supported and merely
        // leaky: the claimant would be reading storage that this destruction is
        // already reclaiming, which is undefined behavior no matter what this
        // destructor does. Doing nothing only avoids compounding that fault
        // with a second run of the sender's and any retained I/O buffer's
        // destructors on a value another thread already owns. The price is that
        // the completion sender is never destroyed and the buffer's memory
        // blocks never return to the bytesbuf pool.
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
/// values are stored directly in `ColdConnectDiagnostics`.
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

/// Stores the WinHTTP secure-failure flags observed for one request.
///
/// TLS validation problems arrive on a `SECURE_FAILURE` callback separate from the
/// `REQUEST_ERROR` that reports the failure itself, so the flags belong to the
/// request rather than to any single operation. Successive callbacks for one request
/// handle run on different WinHTTP worker threads, which is why the storage is
/// atomic.
///
/// The flags occupy the low 32 bits of the packed value and a presence marker sits
/// above them, so a callback that reports no flags stays distinguishable from a
/// request where no such callback arrived.
#[derive(Debug)]
struct SecureFailureDiagnostics {
    packed: AtomicU64,
}

impl SecureFailureDiagnostics {
    /// Marks the packed value as carrying flags from an observed callback.
    const PRESENT: u64 = 1 << u32::BITS;

    const fn new() -> Self {
        Self { packed: AtomicU64::new(0) }
    }

    fn record(&self, flags: u32) {
        self.packed.store(Self::pack(flags), Ordering::Release);
    }

    fn observed(&self) -> Option<u32> {
        let packed = self.packed.load(Ordering::Acquire);

        if packed & Self::PRESENT == 0 {
            return None;
        }

        Some(u32::try_from(packed & u64::from(u32::MAX)).expect("masking to the low 32 bits guarantees a u32 value"))
    }

    /// The presence marker and the flags occupy disjoint bits, so `|` and `^`
    /// compute the same value and a mutation between them is equivalent rather
    /// than a defect.
    #[cfg_attr(test, mutants::skip)] // Disjoint-bit union: `|` and `^` are interchangeable, so operator mutants are equivalent.
    fn pack(flags: u32) -> u64 {
        Self::PRESENT | u64::from(flags)
    }
}

/// Stores connection-establishment progress observed for one request.
///
/// WinHTTP reports connection establishment through callbacks that are independent
/// of operation completions, so the state belongs to the request and is read when a
/// failure needs cold-connection attribution. Successive callbacks for one request
/// handle run on different WinHTTP worker threads, which is why the storage is
/// atomic.
#[derive(Debug)]
struct ColdConnectDiagnostics {
    state: AtomicU8,
}

impl ColdConnectDiagnostics {
    const fn new() -> Self {
        Self {
            state: AtomicU8::new(ColdConnectState::Unobserved as u8),
        }
    }

    fn record(&self, state: ColdConnectState) {
        self.state.store(state as u8, Ordering::Release);
    }

    fn observed(&self) -> ColdConnectState {
        match self.state.load(Ordering::Acquire) {
            value if value == ColdConnectState::Connecting as u8 => ColdConnectState::Connecting,
            value if value == ColdConnectState::Connected as u8 => ColdConnectState::Connected,
            _ => ColdConnectState::Unobserved,
        }
    }
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
    // Records TLS validation flags reported outside the operation slot.
    secure_failure: SecureFailureDiagnostics,
    // Records connection-establishment progress for telemetry attribution.
    cold_connect: ColdConnectDiagnostics,
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
            secure_failure: SecureFailureDiagnostics::new(),
            cold_connect: ColdConnectDiagnostics::new(),
            connect,
            session,
            _pinned: PhantomPinned,
        }
    }

    /// Arms the operation slot for one asynchronous `WinHTTP` call and returns
    /// the receiver endpoint that the request task awaits.
    ///
    /// # Safety
    ///
    /// The context's operation slot must be idle: no operation may be armed
    /// and no claimant may still hold a payload taken from it. The caller must
    /// hold the exclusive `RequestGuard` borrow that establishes both, because
    /// it is the borrow that keeps a second submission from overwriting live
    /// payload and event storage.
    ///
    /// The event armed by a previous call must have reached its terminal
    /// state, because arming reuses the event storage in place: either its
    /// value was received (the `OperationFuture` polled to `Ready`) or its
    /// receiver was destroyed after the sender had already disconnected. The
    /// previous `RawSender` may still be unwinding on a callback thread.
    /// `events_once` ends an event's lifetime at delivery rather than at
    /// endpoint destruction: `EmbeddedEvent` documents that one container may
    /// back multiple events with non-overlapping lifetimes, and the crate's
    /// own pooled events recycle their storage from the receiving poll while
    /// the sender is still returning from `send()`. Demanding that the sender
    /// object itself be gone would be impossible to meet here, because the
    /// callback thread wakes the request task from inside `RawSender::send()`,
    /// so the woken task can receive the value and re-arm before that call
    /// returns.
    ///
    /// The context must also remain pinned at a stable address until both
    /// endpoints of the returned event are destroyed; for an installed context
    /// that means until the final `HANDLE_CLOSING` notification reclaims the
    /// allocation.
    pub(crate) unsafe fn arm(self: Pin<&Self>, kind: OperationKind, buffer: OperationBuffer) -> RawReceiver<CompletionResult> {
        // SAFETY: RequestContext structurally pins its operation field, and the
        // caller guarantees that the context remains pinned through both event
        // endpoints' lifetimes.
        let operation = unsafe { self.map_unchecked(|context| &context.operation) };

        // SAFETY: this method's documented contract states the slot's own
        // requirements verbatim - an idle slot whose previous event reached its
        // terminal state, held under the caller's exclusive RequestGuard
        // borrow, in pinned storage that outlives both new endpoints - so
        // satisfying it satisfies the slot's contract.
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

    pub(crate) fn record_secure_failure(&self, flags: u32) {
        self.secure_failure.record(flags);
    }

    pub(crate) fn secure_failure_flags(&self) -> Option<u32> {
        self.secure_failure.observed()
    }

    pub(crate) fn mark_connecting(&self) {
        self.cold_connect.record(ColdConnectState::Connecting);
    }

    pub(crate) fn mark_connected(&self) {
        self.cold_connect.record(ColdConnectState::Connected);
    }

    pub(crate) fn cold_connect_state(&self) -> ColdConnectState {
        self.cold_connect.observed()
    }
}

impl fmt::Debug for RequestContext {
    #[cfg_attr(coverage_nightly, coverage(off))] // We have no API contract here.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RequestContext")
            .field("operation", &self.operation)
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
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::panic::{RefUnwindSafe, UnwindSafe};

    use bytesbuf::BytesView;
    use bytesbuf::mem::GlobalPool;
    use static_assertions::{assert_impl_all, assert_not_impl_any};
    use windows::Win32::Networking::WinHttp::{
        WINHTTP_ASYNC_RESULT, WINHTTP_CALLBACK_STATUS_CONNECTED_TO_SERVER, WINHTTP_CALLBACK_STATUS_CONNECTING_TO_SERVER,
        WINHTTP_CALLBACK_STATUS_HANDLE_CREATED, WINHTTP_CALLBACK_STATUS_READ_COMPLETE, WINHTTP_CALLBACK_STATUS_REQUEST_ERROR,
        WINHTTP_CALLBACK_STATUS_SECURE_FAILURE,
    };

    use super::{
        ActiveOperation, CallbackOperationSlot, ColdConnectState, CompletionBuffer, CompletionResult, OPERATION_CLAIMED, OPERATION_IDLE,
        OperationBuffer, OperationKind, RequestContext, active_kind,
    };
    use crate::testing::{complete, drive, finish, installed, status_info_len};

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

    #[test]
    fn request_error_classification_uses_error_code_in_both_secure_status_orders() {
        for secure_first in [true, false] {
            let (mut guard, context, contexts, session, closes) = installed();
            let future = guard.submit(OperationKind::SendRequest, OperationBuffer::none(), |_, _| Ok(()));
            let mut secure_flags = 0x20_u32;
            let mut async_result = WINHTTP_ASYNC_RESULT {
                dwResult: 7,
                dwError: 12175,
            };

            if secure_first {
                // SAFETY: complete requires an installed, not-yet-reclaimed
                // context, a payload readable and unmodified for the call, no
                // overlapping notification, no outstanding exclusive borrow,
                // and no use of the context after the reclaiming notification.
                // `installed` returned the pointer it recorded, and only
                // `finish` below reclaims it; the payload is the initialized
                // local `secure_flags`, which outlives the call and nothing
                // else can reach; this test delivers every notification from
                // its own thread; and it borrows the context only sharedly.
                unsafe {
                    complete(
                        context,
                        WINHTTP_CALLBACK_STATUS_SECURE_FAILURE,
                        (&raw mut secure_flags).cast(),
                        status_info_len::<u32>(),
                    );
                }
            }
            // SAFETY: complete requires an installed, not-yet-reclaimed
            // context, a payload readable and unmodified for the call, no
            // overlapping notification, no outstanding exclusive borrow, and no
            // use of the context after the reclaiming notification. `installed`
            // returned the pointer it recorded, and only `finish` below
            // reclaims it; the payload is the initialized local `async_result`,
            // which outlives the call and nothing else can reach; this test
            // delivers every notification from its own thread; and it borrows
            // the context only sharedly.
            unsafe {
                complete(
                    context,
                    WINHTTP_CALLBACK_STATUS_REQUEST_ERROR,
                    (&raw mut async_result).cast(),
                    status_info_len::<WINHTTP_ASYNC_RESULT>(),
                );
            }

            let CompletionResult::Error { error, _buffer: buffer } = drive(future).unwrap() else {
                panic!("request error must produce an error completion");
            };
            assert_eq!(error.code(), 12175);
            assert!(buffer.is_none());
            assert_eq!(error.secure_failure_flags(), secure_first.then_some(0x20));

            if !secure_first {
                // SAFETY: complete requires an installed, not-yet-reclaimed
                // context, a payload readable and unmodified for the call, no
                // overlapping notification, no outstanding exclusive borrow,
                // and no use of the context after the reclaiming notification.
                // `installed` returned the pointer it recorded, and only
                // `finish` below reclaims it; the payload is the initialized
                // local `secure_flags`, which outlives the call and nothing
                // else can reach; this test delivers every notification from
                // its own thread; and it borrows the context only sharedly.
                unsafe {
                    complete(
                        context,
                        WINHTTP_CALLBACK_STATUS_SECURE_FAILURE,
                        (&raw mut secure_flags).cast(),
                        status_info_len::<u32>(),
                    );
                }
                // SAFETY: dereferencing the pointer requires an aligned,
                // initialized context that stays live and free of exclusive
                // borrows for the read. `installed` returned the pointer of the
                // pooled context it built, the guard below is still unconsumed
                // so no reclaiming notification has run, and this borrow is
                // shared and ends with the statement.
                assert_eq!(unsafe { &*context }.secure_failure_flags(), Some(0x20));
            }

            // SAFETY: finish requires an installed, not-yet-reclaimed context,
            // no overlapping notification, and no outstanding exclusive borrow.
            // `installed` returned the pointer it recorded and nothing has
            // reclaimed it, this test delivers every notification from its own
            // thread, and it borrows the context only sharedly.
            unsafe {
                finish(guard, context, &contexts, session, &closes);
            }
        }
    }

    #[test]
    fn a_tag_the_arming_path_cannot_write_names_no_operation() {
        // The tag is the sole authority a callback consults before claiming the
        // payload, so every value it can hold must resolve unambiguously: the
        // exact tag an arming wrote names that operation, and the reserved idle
        // and claimed tags, along with anything no arming could have written,
        // name none.
        for kind in OperationKind::ALL {
            assert_eq!(active_kind(kind as u8), Some(kind));
        }

        assert_eq!(active_kind(OPERATION_IDLE), None);
        assert_eq!(active_kind(OPERATION_CLAIMED), None);
        assert_eq!(active_kind(u8::MAX), None);
    }

    #[test]
    fn an_armed_slot_yields_its_payload_only_to_the_matching_status_and_otherwise_to_destruction() {
        // WinHTTP reports completions for the request handle as a whole, so a
        // notification that does not name the armed operation must leave the
        // payload for the one that does. Destruction is the last claimant: a
        // slot torn down while still armed owns the only completion sender and
        // the only reference to the buffer lent to WinHTTP, and must release
        // both exactly once, which Miri verifies.
        let pool = GlobalPool::new();
        let slot = Box::pin(CallbackOperationSlot::new());
        let buffer = OperationBuffer::write(BytesView::copied_from_slice(b"lent", &pool), 4);

        // SAFETY: arm requires an idle slot whose previous event, if any,
        // reached its terminal state, the exclusive borrow that keeps a second
        // submission from overwriting live storage, and an address that stays
        // stable until both endpoints are destroyed. The slot was just created
        // and is armed once, this test owns it exclusively, and the pinned box
        // holds it in place until the receiver is destroyed below.
        let receiver = unsafe { slot.as_ref().arm(OperationKind::Write, buffer) };

        assert!(slot.take_for_status(WINHTTP_CALLBACK_STATUS_READ_COMPLETE).is_none());

        drop(receiver);
        drop(slot);
    }

    #[test]
    fn connect_attribution_is_bounded_and_handle_created_is_inert() {
        let (guard, context, contexts, session, closes) = installed();

        // SAFETY: complete requires an installed, not-yet-reclaimed context, a
        // payload matching the notification, no overlapping notification, no
        // outstanding exclusive borrow, and no use of the context after the
        // reclaiming notification. `installed` returned the pointer it
        // recorded, and only `finish` below reclaims it; these diagnostic
        // statuses carry no payload, which a null pointer of zero length
        // states; this test delivers every notification from its own thread;
        // and it borrows the context only sharedly.
        unsafe {
            complete(context, WINHTTP_CALLBACK_STATUS_HANDLE_CREATED, std::ptr::null_mut(), 0);
        }
        // SAFETY: dereferencing the pointer requires an aligned, initialized
        // context that stays live and free of exclusive borrows for the read.
        // `installed` returned the pointer of the pooled context it built, the
        // guard below is still unconsumed so no reclaiming notification has
        // run, and this borrow is shared and ends with the statement.
        assert_eq!(unsafe { &*context }.cold_connect_state(), ColdConnectState::Unobserved);

        // SAFETY: as for the preceding payload-free notification, which this
        // test delivers to the same recorded context from the same thread.
        unsafe {
            complete(context, WINHTTP_CALLBACK_STATUS_CONNECTING_TO_SERVER, std::ptr::null_mut(), 0);
        }
        // SAFETY: as for the preceding shared borrow: the guard below still
        // keeps the recorded context from being reclaimed.
        assert_eq!(unsafe { &*context }.cold_connect_state(), ColdConnectState::Connecting);

        // SAFETY: as for the preceding payload-free notification, which this
        // test delivers to the same recorded context from the same thread.
        unsafe {
            complete(context, WINHTTP_CALLBACK_STATUS_CONNECTED_TO_SERVER, std::ptr::null_mut(), 0);
        }
        // SAFETY: as for the preceding shared borrow: the guard below still
        // keeps the recorded context from being reclaimed.
        assert_eq!(unsafe { &*context }.cold_connect_state(), ColdConnectState::Connected);

        // SAFETY: finish requires an installed, not-yet-reclaimed context, no
        // overlapping notification, and no outstanding exclusive borrow.
        // `installed` returned the pointer it recorded and nothing has
        // reclaimed it, this test delivers every notification from its own
        // thread, and it borrows the context only sharedly.
        unsafe {
            finish(guard, context, &contexts, session, &closes);
        }
    }
}
