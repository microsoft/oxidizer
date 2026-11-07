// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

mod discovery;
mod security;

use std::io::{self, Read, Write};
use std::os::windows::io::{AsRawHandle as _, OwnedHandle};
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub(crate) use discovery::{current_directory, registry_directories};
#[cfg(any(test, feature = "test-util"))]
pub(crate) use security::create_directory;
pub(crate) use security::{create_descriptor, current_owner, directory_entries, open_directory, read_descriptor};
use windows_sys::Win32::Foundation::{
    ERROR_BROKEN_PIPE, ERROR_IO_PENDING, ERROR_NO_DATA, ERROR_NOT_FOUND, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED, ERROR_PIPE_NOT_CONNECTED,
    GENERIC_READ, GENERIC_WRITE, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, OPEN_EXISTING, PIPE_ACCESS_DUPLEX, ReadFile, SECURITY_IDENTIFICATION,
    SECURITY_SQOS_PRESENT, WriteFile,
};
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, GetNamedPipeServerProcessId, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
    PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT, PeekNamedPipe,
};
use windows_sys::Win32::System::Threading::{CreateEventW, INFINITE, WaitForSingleObject};

use crate::monitor::{MonitorDescriptor, MonitorEndpoint, MonitorOwner};

const RETRY_DELAY: Duration = Duration::from_millis(2);
// Large snapshots stream through bounded kernel buffers rather than a
// multi-megabyte allocation per client.
const PIPE_BUFFER_BYTES: u32 = 65_536;

#[derive(Debug)]
struct Pipe<H = OwnedHandle> {
    handle: Mutex<Option<Arc<H>>>,
    server: bool,
    closed: AtomicBool,
    #[cfg(any(test, feature = "test-util"))]
    pending_reads: super::PendingReads,
}

impl<H> Pipe<H> {
    fn server(handle: H) -> Self {
        Self::new(handle, true)
    }

    fn new(handle: H, server: bool) -> Self {
        Self {
            handle: Mutex::new(Some(Arc::new(handle))),
            server,
            closed: AtomicBool::new(false),
            #[cfg(any(test, feature = "test-util"))]
            pending_reads: super::PendingReads::default(),
        }
    }

    fn handle(&self) -> io::Result<Arc<H>> {
        self.handle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .cloned()
            .ok_or_else(|| io::ErrorKind::ConnectionAborted.into())
    }
}

impl Pipe {
    #[cfg_attr(coverage_nightly, coverage(off))] // Cancel/disconnect failures require kernel completion races or invalidating a live pipe handle.
    #[cfg_attr(test, mutants::skip)] // Native close/cancellation/clone tests retain observable lifecycle behavior.
    fn close(&self) -> io::Result<()> {
        self.closed.store(true, Ordering::Release);
        let Some(handle) = self.handle.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take() else {
            return Ok(());
        };
        close_pipe_with(
            self.server,
            // SAFETY: the owned handle remains live while pending operations drain.
            || unsafe { CancelIoEx(handle.as_raw_handle(), ptr::null()) },
            // SAFETY: only the server branch invokes this operation on the live handle.
            || unsafe { DisconnectNamedPipe(handle.as_raw_handle()) },
            io::Error::last_os_error,
        )
    }
}

fn close_pipe_with(
    server: bool,
    cancel: impl FnOnce() -> i32,
    disconnect: impl FnOnce() -> i32,
    mut last_error: impl FnMut() -> io::Error,
) -> io::Result<()> {
    if cancel() == 0 {
        let error = last_error();
        if error.raw_os_error() != Some(ERROR_NOT_FOUND.cast_signed()) {
            return Err(error);
        }
    }
    if server && disconnect() == 0 {
        let error = last_error();
        if error.raw_os_error() != Some(ERROR_PIPE_NOT_CONNECTED.cast_signed()) {
            return Err(error);
        }
    }
    Ok(())
}

struct Operation {
    overlapped: super::NativeAllocation<OVERLAPPED>,
    #[cfg(not(test))]
    event: OwnedHandle,
    #[cfg(test)]
    event: Option<OwnedHandle>,
}

impl std::fmt::Debug for Operation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Operation").field("event", &self.event).finish_non_exhaustive()
    }
}

#[expect(
    clippy::non_send_fields_in_send_ty,
    reason = "the raw-owned OVERLAPPED and event outlive native I/O; no pointee references are created while an operation is pending"
)]
// SAFETY: the raw allocation preserves the kernel pointer across owner moves.
// Rust never borrows its pointee while pending; API access is exclusive or
// listener-mutex protected. Completion or cancellation drains all kernel access
// before the allocation and its event are freed, including after a thread move.
unsafe impl Send for Operation {}

impl Operation {
    fn new() -> io::Result<Self> {
        Self::operation_from_event_with(
            |manual_reset, signaled| {
                // SAFETY: an unnamed non-inheritable event needs no external pointers.
                security::owned_handle(unsafe { CreateEventW(ptr::null(), manual_reset, signaled, ptr::null()) })
            },
            |event| {
                let overlapped = super::NativeAllocation::new(OVERLAPPED {
                    hEvent: event.as_raw_handle(),
                    ..OVERLAPPED::default()
                });
                Self {
                    overlapped,
                    #[cfg(not(test))]
                    event,
                    #[cfg(test)]
                    event: Some(event),
                }
            },
        )
    }

    #[cfg_attr(test, mutants::skip)] // This accessor returns owned storage; a null replacement violates the native OVERLAPPED precondition.
    fn pointer(&self) -> *mut OVERLAPPED {
        self.overlapped.as_ptr()
    }

    fn event_handle(&self) -> HANDLE {
        #[cfg(not(test))]
        {
            self.event.as_raw_handle()
        }
        #[cfg(test)]
        {
            self.event
                .as_ref()
                .map_or(ptr::null_mut(), std::os::windows::io::AsRawHandle::as_raw_handle)
        }
    }

    #[cfg_attr(coverage_nightly, coverage(off))] // Kernel cancellation may race completion; draining cannot be observed without live overlapped I/O.
    #[cfg_attr(test, mutants::skip)] // Native cancellation tests retain the lifetime boundary.
    fn cancel_and_drain(&mut self, handle: HANDLE) {
        Self::cancel_operation_with(
            // SAFETY: handle and OVERLAPPED remain live until completion is drained.
            // #[gamma::skip(call_result.default, reason = "cancel_operation_with deliberately ignores cancellation status and always drains; default_after still executes cancellation")]
            || unsafe { cancel_native_operation(handle, self.pointer()) },
            // SAFETY: the policy requests waiting to drain all kernel access before these resources can be freed.
            // #[gamma::skip(call_result.default, reason = "cancel_operation_with ignores the drain status and transferred count after waiting; default_after still performs the drain")]
            |transferred, wait| unsafe { query_native_operation(handle, self.pointer(), transferred, wait) },
        );
    }

    #[cfg_attr(coverage_nightly, coverage(off))] // Completion status is supplied by the kernel, including cancellation races.
    #[cfg_attr(test, mutants::skip)] // Native transfer/cancellation tests retain completed-operation handling.
    fn result(&mut self, handle: HANDLE) -> io::Result<usize> {
        Self::operation_result_with(
            // SAFETY: the completed operation and handle remain live during this query.
            |transferred, wait| unsafe { query_native_operation(handle, self.pointer(), transferred, wait) },
            native_operation_error,
        )
    }

    fn operation_from_event_with<H, O>(create: impl FnOnce(i32, i32) -> io::Result<H>, construct: impl FnOnce(H) -> O) -> io::Result<O> {
        let event = create(
            // #[gamma::skip(literal.int_increment, reason = "the Win32 manual-reset BOOL accepts both one and two as true")]
            1, 0,
        )?;
        Ok(construct(event))
    }

    fn cancel_operation_with(cancel: impl FnOnce() -> i32, drain: impl FnOnce(&mut u32, i32) -> i32) {
        cancel();
        // #[gamma::skip(literal.int_increment, reason = "this cancellation-drain output is never read, regardless of native success or failure")]
        let mut transferred = 0;
        drain(
            &mut transferred,
            // #[gamma::skip(literal.int_increment, reason = "the Win32 BOOL wait flag accepts both one and two as true")]
            1,
        );
    }

    fn operation_result_with(query: impl FnOnce(&mut u32, i32) -> i32, error: impl FnOnce() -> io::Error) -> io::Result<usize> {
        // #[gamma::skip(literal.int_increment, reason = "GetOverlappedResult writes the transferred count on success, and the error branch returns before this output is read")]
        let mut transferred = 0;
        if query(&mut transferred, 0) == 0 {
            return Err(error());
        }
        Ok(transferred as usize)
    }

    #[cfg_attr(coverage_nightly, coverage(off))] // Event failures and all completion/deadline orderings cannot be forced on owned live events.
    #[cfg_attr(test, mutants::skip)] // Native deadline/cancellation tests retain wait behavior.
    fn wait(&mut self, handle: HANDLE, timeout: Option<Duration>) -> io::Result<usize> {
        wait_operation_with(
            self,
            timeout,
            // SAFETY: the event is owned and live throughout the wait.
            |operation, wait| unsafe { WaitForSingleObject(operation.event_handle(), wait) },
            |operation| operation.result(handle),
            |operation| operation.cancel_and_drain(handle),
            io::Error::last_os_error,
        )
    }
}

// #[gamma::skip(fn_value.minus_one, fn_value.one, fn_value.zero, tag = "timeout", reason = "written by cargo gamma suppress 2026-11-06")]
unsafe fn cancel_native_operation(handle: HANDLE, overlapped: *mut OVERLAPPED) -> i32 {
    #[cfg(test)]
    if let Some(hooks) = TEST_NATIVE_OPERATION.get() {
        return (hooks.cancel)(handle, overlapped);
    }
    // SAFETY: The caller keeps the native operation and its handle alive through cancellation.
    // #[gamma::skip(expr.decrement, expr.increment, reason = "native adapter: the kernel BOOL return is not supplied by test hooks; cancellation and subsequent draining are tested through injected operations without pending kernel I/O")]
    unsafe { CancelIoEx(handle, overlapped) }
}

unsafe fn query_native_operation(handle: HANDLE, overlapped: *mut OVERLAPPED, transferred: &mut u32, wait: i32) -> i32 {
    #[cfg(test)]
    if let Some(hooks) = TEST_NATIVE_OPERATION.get() {
        return (hooks.query)(handle, overlapped, transferred, wait);
    }
    // SAFETY: The caller retains the operation and handle; transferred is writable throughout this synchronous query.
    // #[gamma::skip(expr.decrement, expr.increment, reason = "native adapter: mutations of the kernel query and its wait flag require live pending overlapped I/O; injected query tests cover exact wait flags, counts, and errors without claiming to exercise this OS leaf")]
    unsafe { GetOverlappedResult(handle, overlapped, transferred, wait) }
}

fn native_operation_error() -> io::Error {
    #[cfg(test)]
    if let Some(hooks) = TEST_NATIVE_OPERATION.get() {
        return (hooks.error)();
    }
    io::Error::last_os_error()
}

#[cfg(test)]
#[derive(Clone, Copy)]
struct TestNativeOperation {
    cancel: fn(HANDLE, *mut OVERLAPPED) -> i32,
    query: fn(HANDLE, *mut OVERLAPPED, &mut u32, i32) -> i32,
    error: fn() -> io::Error,
}

#[cfg(test)]
thread_local! {
    static TEST_NATIVE_OPERATION: std::cell::Cell<Option<TestNativeOperation>> = const { std::cell::Cell::new(None) };
}

fn wait_operation_with<C>(
    context: &mut C,
    timeout: Option<Duration>,
    wait: impl FnOnce(&mut C, u32) -> u32,
    result: impl FnOnce(&mut C) -> io::Result<usize>,
    cancel: impl FnOnce(&mut C),
    last_error: impl FnOnce() -> io::Error,
) -> io::Result<usize> {
    match wait(context, operation_wait_millis(timeout)) {
        WAIT_OBJECT_0 => result(context),
        WAIT_TIMEOUT => {
            cancel(context);
            Err(io::Error::new(io::ErrorKind::TimedOut, "monitor pipe I/O timed out"))
        }
        _ => {
            let error = last_error();
            cancel(context);
            Err(error)
        }
    }
}

trait AcceptHandle {
    fn raw(&self) -> HANDLE;
}

impl AcceptHandle for Arc<OwnedHandle> {
    fn raw(&self) -> HANDLE {
        self.as_raw_handle()
    }
}

trait AcceptOperation {
    fn wait_event(&mut self, timeout: u32) -> u32;
    fn result(&mut self, handle: HANDLE) -> io::Result<usize>;
    fn cancel_and_drain(&mut self, handle: HANDLE);
    fn last_error(&mut self) -> io::Error;
}

impl AcceptOperation for Operation {
    fn wait_event(&mut self, timeout: u32) -> u32 {
        // SAFETY: the pending operation owns its event throughout this wait.
        unsafe { WaitForSingleObject(self.event_handle(), timeout) }
    }

    fn result(&mut self, handle: HANDLE) -> io::Result<usize> {
        Self::result(self, handle)
    }

    fn cancel_and_drain(&mut self, handle: HANDLE) {
        Self::cancel_and_drain(self, handle);
    }

    fn last_error(&mut self) -> io::Error {
        io::Error::last_os_error()
    }
}

#[derive(Debug)]
struct Accept<H: AcceptHandle = Arc<OwnedHandle>, O: AcceptOperation = Operation> {
    pipe: Arc<Pipe>,
    handle: H,
    operation: O,
    pending: bool,
}

type NativeAcceptParts = (Arc<Pipe>, Arc<OwnedHandle>, Operation, bool);
type NativeAcceptFactory = fn(&MonitorDescriptor, bool) -> io::Result<NativeAcceptParts>;

fn operation_wait_millis(timeout: Option<Duration>) -> u32 {
    timeout.map_or(INFINITE, |duration| {
        u32::try_from(duration.as_millis().saturating_add(1)).unwrap_or(INFINITE - 1)
    })
}

fn named_pipe_parameters(first: bool) -> [u32; 6] {
    [
        PIPE_ACCESS_DUPLEX
            | FILE_FLAG_OVERLAPPED
            | if first {
                FILE_FLAG_FIRST_PIPE_INSTANCE
            } else {
                // #[gamma::skip(literal.int_increment, reason = "PIPE_ACCESS_DUPLEX already sets bit zero, so OR-ing either zero or one in this arm gives the same flags")]
                0
            },
        {
            // #[gamma::skip(bitwise.or_to_and, reason = "PIPE_TYPE_BYTE, PIPE_READMODE_BYTE and PIPE_WAIT are all zero, so OR or AND within this prefix is identically zero")]
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT
        } | PIPE_REJECT_REMOTE_CLIENTS,
        PIPE_UNLIMITED_INSTANCES,
        PIPE_BUFFER_BYTES,
        PIPE_BUFFER_BYTES,
        0,
    ]
}

fn accept_pending(connected: i32, error: impl FnOnce() -> io::Error) -> io::Result<bool> {
    if connected != 0 {
        Ok(false)
    } else {
        let error = error();
        match error.raw_os_error() {
            Some(code) if code == ERROR_PIPE_CONNECTED.cast_signed() => Ok(false),
            Some(code) if code == ERROR_IO_PENDING.cast_signed() => Ok(true),
            _ => Err(error),
        }
    }
}

impl Accept {
    #[cfg_attr(coverage_nightly, coverage(off))] // ConnectNamedPipe immediate/pending/error outcomes depend on client/kernel scheduling.
    #[cfg_attr(test, mutants::skip)] // Native first-instance, authorization, and handshake tests retain pipe creation/accept.
    fn new(descriptor: &MonitorDescriptor, first: bool) -> io::Result<Self> {
        let create: NativeAcceptFactory = create_native_accept;
        #[cfg(test)]
        let create = TEST_ACCEPT_CREATION.get().unwrap_or(create);
        let (pipe, handle, operation, pending) = create(descriptor, first)?;
        Ok(Self {
            pipe,
            handle,
            operation,
            pending,
        })
    }
}

// #[gamma::skip(parameter.default_shadow, reason = "native adapter: creating a protected pipe uses the current token and kernel handles; injected accept creation tests cover first-instance forwarding without invoking this OS leaf")]
fn create_native_accept(descriptor: &MonitorDescriptor, first: bool) -> io::Result<NativeAcceptParts> {
    let MonitorEndpoint::NamedPipe(path) = &descriptor.endpoint else {
        return Err(io::Error::new(io::ErrorKind::Unsupported, "Windows requires a local named pipe"));
    };
    let name = security::wide(path.as_os_str())?;
    let attributes = security::Attributes::for_owner(&descriptor.owner)?;
    create_accept_with(
        first,
        |first| {
            let [flags, mode, instances, output_bytes, input_bytes, timeout] = named_pipe_parameters(first);
            // SAFETY: name and security descriptor are valid for the creation call.
            let handle = unsafe {
                CreateNamedPipeW(
                    name.as_ptr(),
                    flags,
                    mode,
                    instances,
                    output_bytes,
                    input_bytes,
                    timeout,
                    &attributes.native(),
                )
            };
            // #[gamma::skip(try.propagate_to_unwrap, reason = "native adapter: native pipe creation failure cannot be forced without Windows I/O; owned_handle_with tests both invalid sentinels and create_accept_with tests exact creation-error propagation and cleanup")]
            let owned = security::owned_handle(handle)?;
            let pipe = Arc::new(Pipe::server(owned));
            // #[gamma::skip(try.propagate_to_unwrap, reason = "this fresh Pipe contains Some(handle), its mutex has never been exposed, and no other owner can close or poison it before this lookup")]
            let handle = pipe.handle()?;
            Ok((pipe, handle))
        },
        Operation::new,
        // SAFETY: OVERLAPPED has a stable address and outlives any pending accept.
        |handle, operation| unsafe { ConnectNamedPipe(handle.as_raw_handle(), operation.pointer()) },
        io::Error::last_os_error,
    )
}

fn create_accept_with<P, H, O>(
    first: bool,
    create: impl FnOnce(bool) -> io::Result<(P, H)>,
    operation: impl FnOnce() -> io::Result<O>,
    connect: impl FnOnce(&H, &O) -> i32,
    error: impl FnOnce() -> io::Error,
) -> io::Result<(P, H, O, bool)> {
    let (pipe, handle) = create(first)?;
    let operation = operation()?;
    let connected = connect(&handle, &operation);
    let pending = accept_pending(connected, error)?;
    Ok((pipe, handle, operation, pending))
}

impl<H: AcceptHandle, O: AcceptOperation> Accept<H, O> {
    #[cfg_attr(coverage_nightly, coverage(off))] // Owned-event failures and disconnect-before-accept races are kernel-dependent.
    #[cfg_attr(test, mutants::skip)] // Native blocking/nonblocking and abandoned-client tests retain accept behavior.
    fn complete(&mut self, nonblocking: bool) -> io::Result<()> {
        let handle = self.handle.raw();
        complete_accept_with(
            &mut self.pending,
            nonblocking,
            &mut self.operation,
            AcceptOperation::wait_event,
            |operation| operation.result(handle),
            AcceptOperation::last_error,
        )
    }
}

fn complete_accept_with<C>(
    pending: &mut bool,
    nonblocking: bool,
    context: &mut C,
    wait: impl FnOnce(&mut C, u32) -> u32,
    result: impl FnOnce(&mut C) -> io::Result<usize>,
    last_error: impl FnOnce(&mut C) -> io::Error,
) -> io::Result<()> {
    if !*pending {
        return Ok(());
    }
    match wait(context, if nonblocking { 0 } else { INFINITE }) {
        WAIT_TIMEOUT => return Err(io::ErrorKind::WouldBlock.into()),
        WAIT_OBJECT_0 => {}
        _ => return Err(last_error(context)),
    }
    result(context)?;
    // #[gamma::skip(assign_value.default, reason = "The assigned value is already false, exactly bool::default()")]
    *pending = false;
    Ok(())
}

impl<H: AcceptHandle, O: AcceptOperation> Drop for Accept<H, O> {
    fn drop(&mut self) {
        if self.pending {
            self.operation.cancel_and_drain(self.handle.raw());
        }
    }
}

#[derive(Debug)]
pub(crate) struct Listener {
    descriptor: MonitorDescriptor,
    accept: Mutex<Accept>,
    nonblocking: AtomicBool,
}

impl Listener {
    pub(crate) fn bind(descriptor: &MonitorDescriptor) -> io::Result<Self> {
        listener_from_accept_with(
            |first| Accept::new(descriptor, first),
            |accept| Self {
                descriptor: descriptor.clone(),
                accept: Mutex::new(accept),
                nonblocking: AtomicBool::new(false),
            },
        )
    }

    #[expect(
        clippy::unnecessary_wraps,
        reason = "shared listener API also covers fallible Unix socket configuration"
    )]
    pub(crate) fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        self.nonblocking.store(nonblocking, Ordering::Release);
        Ok(())
    }

    #[cfg_attr(coverage_nightly, coverage(off))] // Recycling a disconnected pending instance requires a precise native completion race.
    #[cfg_attr(test, mutants::skip)] // Native multi-client and abandoned-client tests retain instance recycling.
    pub(crate) fn accept(&self) -> io::Result<Connection> {
        let mut accept = self.accept.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        accept_next_with(
            &mut *accept,
            self.nonblocking.load(Ordering::Acquire),
            Accept::complete,
            |first| Accept::new(&self.descriptor, first),
            |connected| Connection::new(Arc::clone(&connected.pipe)),
        )
    }
}

fn listener_from_accept_with<A, L>(create: impl FnOnce(bool) -> io::Result<A>, build: impl FnOnce(A) -> L) -> io::Result<L> {
    Ok(build(create(true)?))
}

fn accept_next_with<A, C>(
    accept: &mut A,
    nonblocking: bool,
    complete: impl FnOnce(&mut A, bool) -> io::Result<()>,
    create: impl FnOnce(bool) -> io::Result<A>,
    connection: impl FnOnce(A) -> C,
) -> io::Result<C> {
    if let Err(error) = complete(accept, nonblocking) {
        if disconnected(&error) {
            let next = create(false)?;
            *accept = next;
            return Err(io::ErrorKind::WouldBlock.into());
        }
        return Err(error);
    }
    // Create the next instance while the original remains open, so there is
    // never a gap in which another process can take the pipe name.
    let next = create(false)?;
    let connected = std::mem::replace(accept, next);
    Ok(connection(connected))
}

#[derive(Debug)]
pub(crate) struct Connection {
    pipe: Arc<Pipe>,
    read_timeout: Mutex<Option<Duration>>,
    write_timeout: Mutex<Option<Duration>>,
}

#[derive(Clone, Copy)]
enum Direction {
    Read,
    Write,
}

impl Connection {
    fn new(pipe: Arc<Pipe>) -> Self {
        Self {
            pipe,
            read_timeout: Mutex::new(None),
            write_timeout: Mutex::new(None),
        }
    }

    pub(crate) fn connect(descriptor: &MonitorDescriptor, timeout: Duration) -> io::Result<Self> {
        let MonitorEndpoint::NamedPipe(path) = &descriptor.endpoint else {
            return Err(io::Error::new(io::ErrorKind::Unsupported, "Windows requires a local named pipe"));
        };
        let name = security::wide(path.as_os_str())?;
        let deadline = Instant::now()
            .checked_add(timeout)
            .filter(|_| !timeout.is_zero())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "connection timeout must be positive and bounded"))?;
        let handle = connect_pipe(&name, deadline)?;
        authenticated_pipe_with(
            descriptor,
            handle,
            pipe_process_id,
            |handle| security::handle_owner(handle.as_raw_handle()),
            |handle| Self::new(Arc::new(Pipe::new(handle, false))),
        )
    }

    #[expect(
        clippy::unnecessary_wraps,
        reason = "shared connection API also covers fallible Unix descriptor duplication"
    )]
    pub(crate) fn try_clone(&self) -> io::Result<Self> {
        Ok(Self {
            pipe: Arc::clone(&self.pipe),
            read_timeout: Mutex::new(*self.read_timeout.lock().unwrap_or_else(std::sync::PoisonError::into_inner)),
            write_timeout: Mutex::new(*self.write_timeout.lock().unwrap_or_else(std::sync::PoisonError::into_inner)),
        })
    }

    pub(crate) fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        check_timeout(timeout)?;
        *self.read_timeout.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = timeout;
        Ok(())
    }

    pub(crate) fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        check_timeout(timeout)?;
        *self.write_timeout.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = timeout;
        Ok(())
    }

    #[cfg_attr(coverage_nightly, coverage(off))] // PeekNamedPipe failures and buffer/deadline races depend on kernel state.
    #[cfg_attr(test, mutants::skip)] // Native peek, disconnect, and deadline tests retain observable behavior.
    pub(crate) fn peek(&self, bytes: &mut [u8]) -> io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        #[cfg(test)]
        if let Some(observe) = TEST_PEEK_TIMEOUT_ACCESS.get() {
            observe();
        }
        let timeout = *self.read_timeout.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let deadline = timeout.and_then(|timeout| Instant::now().checked_add(timeout));
        peek_pipe_with(
            bytes,
            || self.pipe.handle(),
            || self.ensure_open(),
            |handle, bytes, capacity, read, available| {
                // SAFETY: pipe is live and all buffers/count pointers are valid.
                unsafe {
                    PeekNamedPipe(
                        handle.as_raw_handle(),
                        bytes.as_mut_ptr().cast(),
                        capacity,
                        read,
                        available,
                        ptr::null_mut(),
                    )
                }
            },
            io::Error::last_os_error,
            // #[gamma::skip(call.replace_with_default, call_result.default, reason = "native adapter: reaching this retry closure requires an empty live named pipe; injected peek and retry-deadline tests cover timeout policy without kernel I/O or real sleeps")]
            || peek_retry_remaining(deadline, Instant::now),
            std::thread::sleep,
        )
    }

    pub(crate) fn close(&self) -> io::Result<()> {
        self.pipe.close()
    }

    #[cfg(any(test, feature = "test-util"))]
    pub(crate) fn read_is_pending(&self) -> bool {
        self.pipe.pending_reads.active()
    }

    fn ensure_open(&self) -> io::Result<()> {
        if self.pipe.closed.load(Ordering::Acquire) {
            return Err(io::ErrorKind::ConnectionAborted.into());
        }
        Ok(())
    }

    #[cfg_attr(coverage_nightly, coverage(off))] // Immediate/pending I/O, cancellation, and completion orderings depend on native scheduling.
    #[cfg_attr(test, mutants::skip)] // Native transfer/backpressure/deadline/cancellation tests retain the overlapped-I/O boundary.
    fn perform(
        &self,
        direction: Direction,
        timeout: Option<Duration>,
        start: impl FnOnce(HANDLE, *mut OVERLAPPED) -> i32,
    ) -> io::Result<usize> {
        self.ensure_open()?;
        #[cfg(test)]
        if matches!(direction, Direction::Read) && TEST_READ_FILE.get().is_some() {
            let mut overlapped = OVERLAPPED::default();
            return if start(ptr::null_mut(), &raw mut overlapped) != 0 {
                Ok(overlapped.InternalHigh)
            } else {
                Err(io::Error::other("injected native read failure"))
            };
        }
        #[cfg(not(any(test, feature = "test-util")))]
        let _ = direction;
        let handle = self.pipe.handle()?;
        perform_pipe_with(
            timeout,
            Operation::new,
            TransferCallbacks {
                start: |operation: &mut Operation| start(handle.as_raw_handle(), operation.pointer()),
                result: |operation: &mut Operation| operation.result(handle.as_raw_handle()),
                error: io::Error::last_os_error,
                pending: || {
                    #[cfg(any(test, feature = "test-util"))]
                    {
                        matches!(direction, Direction::Read).then(|| self.pipe.pending_reads.enter())
                    }
                },
                closed: || self.pipe.closed.load(Ordering::Acquire),
                cancel: |operation: &mut Operation| operation.cancel_and_drain(handle.as_raw_handle()),
                wait: |operation: &mut Operation, timeout| operation.wait(handle.as_raw_handle(), timeout),
            },
        )
    }
}

fn peek_retry_remaining(deadline: Option<Instant>, now: impl FnOnce() -> Instant) -> Option<Duration> {
    deadline.map_or(Some(RETRY_DELAY), |deadline| deadline.checked_duration_since(now()))
}

fn pipe_buffer_length(length: usize) -> u32 {
    u32::try_from(length).unwrap_or(u32::MAX)
}

fn peek_pipe_with<H>(
    bytes: &mut [u8],
    handle: impl FnOnce() -> io::Result<H>,
    mut ensure_open: impl FnMut() -> io::Result<()>,
    mut query: impl FnMut(&H, &mut [u8], u32, &mut u32, &mut u32) -> i32,
    mut error: impl FnMut() -> io::Error,
    mut remaining: impl FnMut() -> Option<Duration>,
    mut sleep: impl FnMut(Duration),
) -> io::Result<usize> {
    if bytes.is_empty() {
        return Ok(0);
    }
    let handle = handle()?;
    loop {
        ensure_open()?;
        // #[gamma::skip(literal.int_increment, reason = "PeekNamedPipe overwrites bytes-read on success and the error branch never reads this output")]
        let mut read = 0;
        // #[gamma::skip(literal.int_increment, reason = "PeekNamedPipe overwrites bytes-available on success and the error branch never reads this output")]
        let mut available = 0;
        let result = query(&handle, bytes, pipe_buffer_length(bytes.len()), &mut read, &mut available);
        if result == 0 {
            let error = error();
            return if disconnected(&error) { Ok(0) } else { Err(error) };
        }
        if available != 0 {
            return Ok(read as usize);
        }
        let remaining = remaining().ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "monitor pipe peek timed out"))?;
        sleep(remaining.min(RETRY_DELAY));
    }
}

struct TransferCallbacks<S, R, E, P, C, A, W> {
    start: S,
    result: R,
    error: E,
    pending: P,
    closed: C,
    cancel: A,
    wait: W,
}

fn perform_pipe_with<O, G, S, R, E, P, C, A, W>(
    timeout: Option<Duration>,
    create: impl FnOnce() -> io::Result<O>,
    callbacks: TransferCallbacks<S, R, E, P, C, A, W>,
) -> io::Result<usize>
where
    S: FnOnce(&mut O) -> i32,
    R: FnOnce(&mut O) -> io::Result<usize>,
    E: FnOnce() -> io::Error,
    P: FnOnce() -> G,
    C: FnOnce() -> bool,
    A: FnOnce(&mut O),
    W: FnOnce(&mut O, Option<Duration>) -> io::Result<usize>,
{
    let TransferCallbacks {
        start,
        result,
        error,
        pending,
        closed,
        cancel,
        wait,
    } = callbacks;
    let mut operation = create()?;
    if start(&mut operation) != 0 {
        return result(&mut operation);
    }
    let error = error();
    if
    // #[gamma::skip(cond.always_false, tag = "timeout", reason = "November 4 campaign: this mutation exceeded the test timeout")]
    error.raw_os_error() != Some(ERROR_IO_PENDING.cast_signed()) {
        return if disconnected(&error) { Ok(0) } else { Err(error) };
    }
    let _pending = pending();
    if closed() {
        cancel(&mut operation);
        return Err(io::ErrorKind::ConnectionAborted.into());
    }
    wait(&mut operation, timeout).or_else(|error| if disconnected(&error) { Ok(0) } else { Err(error) })
}

#[cfg_attr(coverage_nightly, coverage(off))] // Busy-pipe/deadline orderings and native open failures require external pipe/kernel state.
#[cfg_attr(test, mutants::skip)] // Native connect/authorization tests retain local-only, identification-level opens.
fn connect_pipe(name: &[u16], deadline: Instant) -> io::Result<OwnedHandle> {
    connect_pipe_with(
        name,
        |name, access, share, disposition, flags| {
            // SAFETY: name is terminated and the returned handle is non-inheritable.
            security::owned_handle(unsafe { CreateFileW(name.as_ptr(), access, share, ptr::null(), disposition, flags, ptr::null_mut()) })
        },
        || deadline.checked_duration_since(Instant::now()),
        std::thread::sleep,
    )
}

fn connect_pipe_with<H>(
    name: &[u16],
    mut open: impl FnMut(&[u16], u32, u32, u32, u32) -> io::Result<H>,
    mut remaining: impl FnMut() -> Option<Duration>,
    mut sleep: impl FnMut(Duration),
) -> io::Result<H> {
    loop {
        // Identification permits the server to inspect the client identity,
        // never to borrow an elevated client's privileges.
        match open(
            name,
            GENERIC_READ | GENERIC_WRITE,
            0,
            OPEN_EXISTING,
            FILE_FLAG_OVERLAPPED | SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
        ) {
            Ok(handle) => return Ok(handle),
            Err(error) if error.raw_os_error() == Some(ERROR_PIPE_BUSY.cast_signed()) => {
                let remaining = remaining().ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "monitor pipe connection timed out"))?;
                sleep(remaining.min(RETRY_DELAY));
            }
            Err(error) => return Err(error),
        }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))] // Server-PID query failure cannot be injected on a valid connected local pipe.
#[cfg_attr(test, mutants::skip)] // Native mismatched-PID tests retain the identity check outside this wrapper.
fn pipe_process_id(handle: &OwnedHandle) -> io::Result<u32> {
    pipe_process_id_with(
        // SAFETY: handle is a live connected pipe and process_id is writable.
        |process_id| unsafe { GetNamedPipeServerProcessId(handle.as_raw_handle(), process_id) },
        io::Error::last_os_error,
    )
}

fn pipe_process_id_with(query: impl FnOnce(&mut u32) -> i32, error: impl FnOnce() -> io::Error) -> io::Result<u32> {
    // #[gamma::skip(literal.int_increment, reason = "GetNamedPipeServerProcessId overwrites its output on success, and failure returns before the output is read")]
    let mut process_id = 0;
    if query(&mut process_id) == 0 {
        return Err(error());
    }
    Ok(process_id)
}

fn authenticated_pipe_with<H, C>(
    descriptor: &MonitorDescriptor,
    handle: H,
    process_id: impl FnOnce(&H) -> io::Result<u32>,
    owner: impl FnOnce(&H) -> io::Result<MonitorOwner>,
    connection: impl FnOnce(H) -> C,
) -> io::Result<C> {
    if process_id(&handle)? != descriptor.process_id || owner(&handle)? != descriptor.owner {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "connected monitor server has a different OS identity",
        ));
    }
    Ok(connection(handle))
}

impl Read for Connection {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let timeout = *self.read_timeout.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        self.perform(Direction::Read, timeout, |handle, overlapped| {
            let read = ReadFile;
            #[cfg(test)]
            let read: NativeReadFile = TEST_READ_FILE.get().unwrap_or(read);
            // SAFETY: buf and OVERLAPPED remain live until perform drains the
            // operation, including on cancellation or a deadline.
            unsafe { read(handle, buf.as_mut_ptr(), pipe_buffer_length(buf.len()), ptr::null_mut(), overlapped) }
        })
    }
}

#[cfg(test)]
type NativeReadFile = unsafe extern "system" fn(HANDLE, *mut u8, u32, *mut u32, *mut OVERLAPPED) -> i32;

#[cfg(test)]
thread_local! {
    static TEST_ACCEPT_CREATION: std::cell::Cell<Option<NativeAcceptFactory>> = const { std::cell::Cell::new(None) };
    static TEST_READ_FILE: std::cell::Cell<Option<NativeReadFile>> = const { std::cell::Cell::new(None) };
    static TEST_PEEK_TIMEOUT_ACCESS: std::cell::Cell<Option<fn()>> = const { std::cell::Cell::new(None) };
}

impl Write for Connection {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let timeout = *self.write_timeout.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        self.perform(Direction::Write, timeout, |handle, overlapped| {
            // SAFETY: buf and OVERLAPPED remain live until perform drains the operation.
            unsafe { WriteFile(handle, buf.as_ptr(), pipe_buffer_length(buf.len()), ptr::null_mut(), overlapped) }
        })
    }

    fn flush(&mut self) -> io::Result<()> {
        // Writes already reach the pipe's kernel buffer. FlushFileBuffers would
        // instead wait for peer consumption without our cancellation deadline.
        self.ensure_open()
    }
}

fn check_timeout(timeout: Option<Duration>) -> io::Result<()> {
    if timeout.is_some_and(|timeout| timeout.is_zero() || Instant::now().checked_add(timeout).is_none()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "monitor I/O timeout must be positive and bounded",
        ));
    }
    Ok(())
}

fn disconnected(error: &io::Error) -> bool {
    matches!(
        error.raw_os_error(),
        Some(code) if code == ERROR_BROKEN_PIPE.cast_signed() || code == ERROR_NO_DATA.cast_signed() || code == ERROR_PIPE_NOT_CONNECTED.cast_signed()
    )
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #[test]
    fn nov6_actual_accept_and_listener_preserve_creation_errors_and_first_instance_without_native_handles() {
        struct Restore(Option<NativeAcceptFactory>);
        impl Drop for Restore {
            fn drop(&mut self) {
                TEST_ACCEPT_CREATION.set(self.0);
            }
        }
        fn reject(descriptor: &MonitorDescriptor, first: bool) -> io::Result<NativeAcceptParts> {
            assert_eq!(descriptor.name, "creation probe");
            assert_eq!(descriptor.process_id, 17);
            assert_eq!(descriptor.instance.as_deref(), Some(if first { "true" } else { "false" }));
            Err(io::Error::new(io::ErrorKind::PermissionDenied, "injected accept creation failure"))
        }
        let _restore = Restore(TEST_ACCEPT_CREATION.replace(Some(reject)));
        let mut descriptor = MonitorDescriptor {
            name: "creation probe".into(),
            instance: None,
            process_id: 17,
            instance_id: crate::monitor::InstanceId::from_bytes([17; 16]),
            endpoint: MonitorEndpoint::NamedPipe("not opened".into()),
            owner: MonitorOwner::WindowsSid("S-1-5-18".into()),
        };
        for first in [false, true] {
            descriptor.instance = Some(first.to_string());
            let error = Accept::new(&descriptor, first).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
            assert_eq!(error.to_string(), "injected accept creation failure");
        }
        descriptor.instance = Some("true".into());
        let error = Listener::bind(&descriptor).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(error.to_string(), "injected accept creation failure");
    }

    #[test]
    fn nov6_server_pipe_keeps_its_role_and_fake_handle_until_the_final_owner_drops() {
        struct Handle {
            identity: u32,
            drops: Rc<Cell<usize>>,
        }
        impl Drop for Handle {
            fn drop(&mut self) {
                self.drops.set(self.drops.get() + 1);
            }
        }
        let drops = Rc::new(Cell::new(0));
        let pipe = Pipe::server(Handle {
            identity: 37,
            drops: Rc::clone(&drops),
        });
        assert!(pipe.server);
        assert!(!pipe.closed.load(Ordering::Acquire));
        let handle = pipe.handle().unwrap();
        assert_eq!(handle.identity, 37);
        assert_eq!(Arc::strong_count(&handle), 2);
        assert_eq!(drops.get(), 0);
        drop(pipe);
        assert_eq!(drops.get(), 0);
        assert_eq!(Arc::strong_count(&handle), 1);
        drop(handle);
        assert_eq!(drops.get(), 1);
    }

    #[test]
    fn nov6_empty_native_peek_returns_before_timeout_lock_or_clock_access() {
        struct Restore(Option<fn()>);
        impl Drop for Restore {
            fn drop(&mut self) {
                TEST_PEEK_TIMEOUT_ACCESS.set(self.0);
            }
        }
        let _restore =
            Restore(TEST_PEEK_TIMEOUT_ACCESS.replace(Some(|| panic!("an empty peek must not access the timeout mutex or clock"))));
        let connection = Connection::new(Arc::new(Pipe {
            handle: Mutex::new(None),
            server: false,
            closed: AtomicBool::new(false),
            pending_reads: super::super::PendingReads::default(),
        }));
        assert_eq!(connection.peek(&mut []).unwrap(), 0);
    }

    #[test]
    fn nov6_peek_retry_default_and_expiration_are_exact_without_observing_a_clock() {
        assert_eq!(
            peek_retry_remaining(None, || panic!("an unlimited peek must not observe time")),
            Some(RETRY_DELAY)
        );
        let (io, _) = crate::io::MonitorIo::new_mocked();
        let now = io.instant();
        let deadline = now.checked_add(Duration::from_secs(3)).unwrap();
        assert_eq!(peek_retry_remaining(Some(deadline), || now), Some(Duration::from_secs(3)));
        assert_eq!(peek_retry_remaining(Some(deadline), || deadline), Some(Duration::ZERO));
        assert_eq!(
            peek_retry_remaining(Some(deadline), || deadline.checked_add(Duration::from_secs(1)).unwrap()),
            None
        );
    }

    #[test]
    fn nov6_native_read_forwards_the_exact_writable_capacity_and_reports_the_completed_extent() {
        struct Restore(Option<NativeReadFile>);
        impl Drop for Restore {
            fn drop(&mut self) {
                TEST_READ_FILE.set(self.0);
            }
        }
        unsafe extern "system" fn read(handle: HANDLE, bytes: *mut u8, length: u32, count: *mut u32, overlapped: *mut OVERLAPPED) -> i32 {
            assert!(handle.is_null());
            assert!(count.is_null());
            assert_eq!(length, 3);
            // SAFETY: The caller supplies exactly three writable bytes to this synchronous fake.
            unsafe { bytes.copy_from_nonoverlapping(b"\x12\x34\x56".as_ptr(), 3) };
            // SAFETY: The synchronous fake retains the caller's live writable OVERLAPPED storage.
            unsafe { (*overlapped).InternalHigh = 3 };
            1
        }
        let _restore = Restore(TEST_READ_FILE.replace(Some(read)));
        let mut connection = Connection::new(Arc::new(Pipe {
            handle: Mutex::new(None),
            server: false,
            closed: AtomicBool::new(false),
            pending_reads: super::super::PendingReads::default(),
        }));
        let mut bytes = [0; 3];
        assert_eq!(connection.read(&mut bytes).unwrap(), 3);
        assert_eq!(bytes, [0x12, 0x34, 0x56]);
    }

    #[test]
    fn nov6_actual_operation_adapters_query_exact_results_and_cancel_before_waiting_for_drain() {
        thread_local! {
            static EVENTS: RefCell<Vec<&'static str>> = const { RefCell::new(Vec::new()) };
            static SUCCEED: Cell<bool> = const { Cell::new(true) };
        }
        struct Restore(Option<TestNativeOperation>);
        impl Drop for Restore {
            fn drop(&mut self) {
                TEST_NATIVE_OPERATION.set(self.0);
            }
        }
        EVENTS.with_borrow_mut(Vec::clear);
        let _restore = Restore(TEST_NATIVE_OPERATION.replace(Some(TestNativeOperation {
            cancel: |handle, overlapped| {
                assert!(handle.is_null());
                assert!(!overlapped.is_null());
                EVENTS.with_borrow_mut(|events| events.push("cancel"));
                1
            },
            query: |handle, overlapped, transferred, wait| {
                assert!(handle.is_null());
                assert!(!overlapped.is_null());
                assert_eq!(*transferred, 0);
                EVENTS.with_borrow_mut(|events| {
                    events.push(if wait == 0 {
                        "result"
                    } else {
                        assert_eq!(wait, 1);
                        "drain"
                    });
                });
                *transferred = 0x1234;
                i32::from(SUCCEED.get())
            },
            error: || io::Error::from_raw_os_error(5),
        })));
        let mut operation = Operation {
            overlapped: super::super::NativeAllocation::new(OVERLAPPED::default()),
            event: None,
        };
        SUCCEED.set(true);
        assert_eq!(AcceptOperation::result(&mut operation, ptr::null_mut()).unwrap(), 0x1234);
        SUCCEED.set(false);
        assert_eq!(
            AcceptOperation::result(&mut operation, ptr::null_mut()).unwrap_err().raw_os_error(),
            Some(5)
        );
        SUCCEED.set(true);
        AcceptOperation::cancel_and_drain(&mut operation, ptr::null_mut());
        assert_eq!(EVENTS.with_borrow(Clone::clone), ["result", "result", "cancel", "drain"]);
    }

    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    #[test]
    fn november_accept_creation_preserves_instance_flags_and_cleanup_across_each_failure_stage() {
        for first in [false, true] {
            for case in [
                "create-error",
                "operation-error",
                "connect-error",
                "immediate",
                "connected",
                "pending",
            ] {
                assert_accept_creation(first, case);
            }
        }
    }

    fn assert_accept_creation(first: bool, case: &str) {
        let events = RefCell::new(Vec::new());
        let result = create_accept_with(
            first,
            |actual_first| {
                assert_eq!(actual_first, first);
                events.borrow_mut().push("create");
                if case == "create-error" {
                    Err(io::Error::other("pipe refused"))
                } else {
                    Ok((
                        TransferTestResource {
                            drop_event: "pipe-drop",
                            events: &events,
                        },
                        TransferTestResource {
                            drop_event: "handle-drop",
                            events: &events,
                        },
                    ))
                }
            },
            || {
                assert_eq!(*events.borrow(), ["create"]);
                events.borrow_mut().push("operation");
                if case == "operation-error" {
                    Err(io::Error::other("event refused"))
                } else {
                    Ok(TransferTestResource {
                        drop_event: "operation-drop",
                        events: &events,
                    })
                }
            },
            |handle, operation| {
                assert_eq!(handle.drop_event, "handle-drop");
                assert_eq!(operation.drop_event, "operation-drop");
                assert_eq!(*events.borrow(), ["create", "operation"]);
                events.borrow_mut().push("connect");
                i32::from(case == "immediate")
            },
            || {
                assert_ne!(case, "immediate");
                events.borrow_mut().push("error");
                io::Error::from_raw_os_error(match case {
                    "connected" => ERROR_PIPE_CONNECTED.cast_signed(),
                    "pending" => ERROR_IO_PENDING.cast_signed(),
                    _ => 5,
                })
            },
        );
        if case.ends_with("-error") {
            let error = result.unwrap_err();
            match case {
                "create-error" => {
                    assert_eq!(error.to_string(), "pipe refused");
                    assert_eq!(*events.borrow(), ["create"]);
                }
                "operation-error" => {
                    assert_eq!(error.to_string(), "event refused");
                    assert_eq!(*events.borrow(), ["create", "operation", "handle-drop", "pipe-drop"]);
                }
                _ => {
                    assert_eq!(error.raw_os_error(), Some(5));
                    assert_eq!(
                        *events.borrow(),
                        [
                            "create",
                            "operation",
                            "connect",
                            "error",
                            "operation-drop",
                            "handle-drop",
                            "pipe-drop"
                        ]
                    );
                }
            }
        } else {
            let owned = result.unwrap();
            assert_eq!(owned.3, case == "pending");
            let mut expected = vec!["create", "operation", "connect"];
            if case != "immediate" {
                expected.push("error");
            }
            assert_eq!(*events.borrow(), expected);
            drop(owned);
            expected.extend(["pipe-drop", "handle-drop", "operation-drop"]);
            assert_eq!(*events.borrow(), expected);
        }
    }

    #[test]
    fn november_listener_creation_requests_first_instance_and_propagates_creation_failure_before_build() {
        for fail in [false, true] {
            let built = Cell::new(false);
            let result = listener_from_accept_with(
                |first| {
                    assert!(first);
                    if fail { Err(io::Error::other("listener refused")) } else { Ok(17) }
                },
                |accept| {
                    assert_eq!(accept, 17);
                    built.set(true);
                    accept + 1
                },
            );
            assert_eq!(built.get(), !fail);
            if fail {
                assert_eq!(result.unwrap_err().to_string(), "listener refused");
            } else {
                assert_eq!(result.unwrap(), 18);
            }
        }
    }

    #[test]
    fn november_listener_recycling_preserves_old_ownership_until_replacement_is_ready() {
        for nonblocking in [false, true] {
            for case in ["success", "broken", "ordinary", "create-success-error", "create-broken-error"] {
                let events = RefCell::new(Vec::new());
                let mut accept = TransferTestResource {
                    drop_event: "old",
                    events: &events,
                };
                let result = accept_next_with(
                    &mut accept,
                    nonblocking,
                    |current, actual_nonblocking| {
                        assert_eq!(current.drop_event, "old");
                        assert_eq!(actual_nonblocking, nonblocking);
                        events.borrow_mut().push("complete");
                        match case {
                            "broken" | "create-broken-error" => Err(io::Error::from_raw_os_error(ERROR_BROKEN_PIPE.cast_signed())),
                            "ordinary" => Err(io::Error::from_raw_os_error(5)),
                            _ => Ok(()),
                        }
                    },
                    |first| {
                        assert!(!first);
                        assert_eq!(*events.borrow(), ["complete"]);
                        events.borrow_mut().push("create");
                        if case.starts_with("create-") {
                            Err(io::Error::other("replacement refused"))
                        } else {
                            Ok(TransferTestResource {
                                drop_event: "next",
                                events: &events,
                            })
                        }
                    },
                    |connected| {
                        assert_eq!(*events.borrow(), ["complete", "create"]);
                        events.borrow_mut().push("build");
                        connected
                    },
                );
                match case {
                    "success" => {
                        assert_eq!(accept.drop_event, "next");
                        assert_eq!(*events.borrow(), ["complete", "create", "build"]);
                        drop(result.unwrap());
                        assert_eq!(*events.borrow(), ["complete", "create", "build", "old"]);
                    }
                    "broken" => {
                        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::WouldBlock);
                        assert_eq!(accept.drop_event, "next");
                        assert_eq!(*events.borrow(), ["complete", "create", "old"]);
                    }
                    "ordinary" => {
                        assert_eq!(result.unwrap_err().raw_os_error(), Some(5));
                        assert_eq!(accept.drop_event, "old");
                        assert_eq!(*events.borrow(), ["complete"]);
                    }
                    _ => {
                        assert_eq!(result.unwrap_err().to_string(), "replacement refused");
                        assert_eq!(accept.drop_event, "old");
                        assert_eq!(*events.borrow(), ["complete", "create"]);
                    }
                }
                let retained = accept.drop_event;
                drop(accept);
                assert_eq!(events.borrow().last(), Some(&retained));
            }
        }
    }

    #[test]
    fn november_pipe_authentication_short_circuits_mismatched_pid_and_releases_every_rejected_handle() {
        let descriptor = MonitorDescriptor {
            name: "no native I/O".into(),
            instance: None,
            process_id: 17,
            instance_id: crate::monitor::InstanceId::from_bytes([17; 16]),
            endpoint: MonitorEndpoint::NamedPipe("not opened".into()),
            owner: MonitorOwner::WindowsSid("S-1-5-18".into()),
        };
        for case in ["success", "pid-mismatch", "owner-mismatch", "pid-error", "owner-error"] {
            let events = RefCell::new(Vec::new());
            let result = authenticated_pipe_with(
                &descriptor,
                TransferTestResource {
                    drop_event: "handle",
                    events: &events,
                },
                |_| {
                    events.borrow_mut().push("pid");
                    if case == "pid-error" {
                        Err(io::Error::from_raw_os_error(5))
                    } else {
                        Ok(if case == "pid-mismatch" { 18 } else { 17 })
                    }
                },
                |_| {
                    assert!(!case.starts_with("pid-"));
                    events.borrow_mut().push("owner");
                    if case == "owner-error" {
                        Err(io::Error::from_raw_os_error(6))
                    } else {
                        Ok(MonitorOwner::WindowsSid(
                            if case == "owner-mismatch" { "S-1-5-19" } else { "S-1-5-18" }.into(),
                        ))
                    }
                },
                |handle| {
                    assert_eq!(*events.borrow(), ["pid", "owner"]);
                    events.borrow_mut().push("build");
                    handle
                },
            );
            if case == "success" {
                let handle = result.unwrap();
                assert_eq!(*events.borrow(), ["pid", "owner", "build"]);
                drop(handle);
                assert_eq!(*events.borrow(), ["pid", "owner", "build", "handle"]);
            } else {
                let error = result.unwrap_err();
                if case.ends_with("-mismatch") {
                    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
                    assert_eq!(error.to_string(), "connected monitor server has a different OS identity");
                } else {
                    assert_eq!(error.raw_os_error(), Some(if case == "pid-error" { 5 } else { 6 }));
                }
                assert_eq!(
                    *events.borrow(),
                    if case.starts_with("pid-") {
                        vec!["pid", "handle"]
                    } else {
                        vec!["pid", "owner", "handle"]
                    }
                );
            }
        }
    }

    #[test]
    fn november_process_id_query_preserves_the_full_output_and_exact_failure_without_reading_failed_output() {
        for count in [0, 17, u32::MAX] {
            for status in [-1, 0, 1] {
                let result = pipe_process_id_with(
                    |output| {
                        *output = count;
                        status
                    },
                    || {
                        assert_eq!(status, 0);
                        io::Error::from_raw_os_error(5)
                    },
                );
                if status == 0 {
                    assert_eq!(result.unwrap_err().raw_os_error(), Some(5));
                } else {
                    assert_eq!(result.unwrap(), count);
                }
            }
        }
    }

    #[derive(Debug, Default)]
    struct AcceptCalls {
        waits: Vec<u32>,
        results: usize,
        errors: usize,
        cancellations: usize,
    }

    #[derive(Debug)]
    struct TestAcceptOperation {
        calls: Rc<RefCell<AcceptCalls>>,
        status: u32,
        result_error: bool,
    }

    impl AcceptHandle for () {
        fn raw(&self) -> HANDLE {
            ptr::dangling_mut()
        }
    }

    impl AcceptOperation for TestAcceptOperation {
        fn wait_event(&mut self, timeout: u32) -> u32 {
            self.calls.borrow_mut().waits.push(timeout);
            self.status
        }

        fn result(&mut self, handle: HANDLE) -> io::Result<usize> {
            assert_eq!(handle, ptr::dangling_mut());
            self.calls.borrow_mut().results += 1;
            if self.result_error {
                Err(io::Error::from_raw_os_error(6))
            } else {
                Ok(17)
            }
        }

        fn cancel_and_drain(&mut self, handle: HANDLE) {
            assert_eq!(handle, ptr::dangling_mut());
            self.calls.borrow_mut().cancellations += 1;
        }

        fn last_error(&mut self) -> io::Error {
            self.calls.borrow_mut().errors += 1;
            io::Error::from_raw_os_error(5)
        }
    }

    #[test]
    fn november_actual_accept_completion_and_drop_preserve_pending_ownership_for_every_wait_outcome() {
        for pending in [false, true] {
            for nonblocking in [false, true] {
                for status in [WAIT_OBJECT_0, WAIT_TIMEOUT, u32::MAX] {
                    for result_error in [false, true] {
                        assert_actual_accept(pending, nonblocking, status, result_error);
                    }
                }
            }
        }
    }

    fn assert_actual_accept(pending: bool, nonblocking: bool, status: u32, result_error: bool) {
        let calls = Rc::new(RefCell::new(AcceptCalls::default()));
        let pipe = Arc::new(Pipe {
            handle: Mutex::new(None),
            server: true,
            closed: AtomicBool::new(false),
            pending_reads: super::super::PendingReads::default(),
        });
        let mut accept = Accept {
            pipe: Arc::clone(&pipe),
            handle: (),
            operation: TestAcceptOperation {
                calls: Rc::clone(&calls),
                status,
                result_error,
            },
            pending,
        };
        let result = accept.complete(nonblocking);
        let remains_pending = pending && (status != WAIT_OBJECT_0 || result_error);
        assert_eq!(accept.pending, remains_pending);
        assert!(Arc::ptr_eq(&accept.pipe, &pipe));
        if !pending || !remains_pending {
            result.unwrap();
        } else {
            let error = result.unwrap_err();
            if status == WAIT_TIMEOUT {
                assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
                assert_eq!(error.to_string(), "operation would block");
            } else {
                assert_eq!(error.raw_os_error(), Some(if status == WAIT_OBJECT_0 { 6 } else { 5 }));
            }
        }
        assert_eq!(
            calls.borrow().waits,
            if pending {
                vec![if nonblocking { 0 } else { INFINITE }]
            } else {
                vec![]
            }
        );
        assert_eq!(calls.borrow().results, usize::from(pending && status == WAIT_OBJECT_0));
        assert_eq!(
            calls.borrow().errors,
            usize::from(pending && status != WAIT_OBJECT_0 && status != WAIT_TIMEOUT)
        );
        assert_eq!(calls.borrow().cancellations, 0);
        drop(accept);
        assert_eq!(calls.borrow().cancellations, usize::from(remains_pending));
        assert_eq!(Arc::strong_count(&pipe), 1);
    }

    #[test]
    fn november_actual_pipe_constructor_preserves_role_and_shared_resource_ownership_without_native_handles() {
        for server in [false, true] {
            let events = RefCell::new(Vec::new());
            let pipe = Pipe::new(
                TransferTestResource {
                    drop_event: "handle",
                    events: &events,
                },
                server,
            );
            assert_eq!(pipe.server, server);
            assert!(!pipe.closed.load(Ordering::Acquire));
            let handle = pipe.handle().unwrap();
            assert_eq!(Arc::strong_count(&handle), 2);
            drop(pipe);
            assert!(events.borrow().is_empty());
            drop(handle);
            assert_eq!(*events.borrow(), ["handle"]);
        }
    }

    #[test]
    fn november_event_creation_requests_manual_reset_unsignaled_and_retains_ownership_only_after_success() {
        for fail in [false, true] {
            let events = RefCell::new(Vec::new());
            let result = Operation::operation_from_event_with(
                |manual, signaled| {
                    assert_ne!(manual, 0);
                    assert_eq!(signaled, 0);
                    events.borrow_mut().push("create");
                    if fail {
                        Err(io::Error::other("event refused"))
                    } else {
                        Ok(TransferTestResource {
                            drop_event: "event",
                            events: &events,
                        })
                    }
                },
                |event| {
                    events.borrow_mut().push("construct");
                    event
                },
            );
            if fail {
                assert_eq!(result.unwrap_err().to_string(), "event refused");
                assert_eq!(*events.borrow(), ["create"]);
            } else {
                let event = result.unwrap();
                assert_eq!(*events.borrow(), ["create", "construct"]);
                drop(event);
                assert_eq!(*events.borrow(), ["create", "construct", "event"]);
            }
        }
    }

    #[test]
    fn november_native_result_policy_is_nonblocking_and_reads_count_only_after_success() {
        for count in [0, 3, u32::MAX] {
            assert_eq!(
                Operation::operation_result_with(
                    |output, wait| {
                        assert_eq!(wait, 0);
                        *output = count;
                        1
                    },
                    || panic!("successful query must not obtain an error"),
                )
                .unwrap(),
                count as usize
            );
        }
        let error = Operation::operation_result_with(
            |output, wait| {
                assert_eq!(wait, 0);
                *output = u32::MAX;
                0
            },
            || io::Error::from_raw_os_error(6),
        )
        .unwrap_err();
        assert_eq!(error.raw_os_error(), Some(6));
    }

    #[test]
    fn november_cancellation_always_drains_with_wait_even_when_cancellation_or_drain_reports_failure() {
        for cancelled in [0, 1] {
            for drained in [0, 1] {
                let events = RefCell::new(Vec::new());
                Operation::cancel_operation_with(
                    || {
                        events.borrow_mut().push("cancel");
                        cancelled
                    },
                    |output, wait| {
                        assert_ne!(wait, 0);
                        assert_eq!(*events.borrow(), ["cancel"]);
                        *output = 23;
                        events.borrow_mut().push("drain");
                        drained
                    },
                );
                assert_eq!(*events.borrow(), ["cancel", "drain"]);
            }
        }
    }

    #[derive(Debug)]
    struct TransferTestResource<'a> {
        drop_event: &'static str,
        events: &'a RefCell<Vec<&'static str>>,
    }

    impl Drop for TransferTestResource<'_> {
        fn drop(&mut self) {
            self.events.borrow_mut().push(self.drop_event);
        }
    }

    #[test]
    fn november_transfer_policy_owns_pending_operations_through_completion_or_cancellation() {
        for case in [
            "create-error",
            "immediate",
            "immediate-error",
            "failed-start",
            "disconnected-start",
            "closed",
            "pending",
            "wait-disconnect",
            "wait-error",
        ] {
            let events = RefCell::new(Vec::new());
            let timeout = Some(Duration::from_nanos(19));
            let result = perform_pipe_with(
                timeout,
                || {
                    events.borrow_mut().push("create");
                    if case == "create-error" {
                        Err(io::Error::other("create refused"))
                    } else {
                        Ok(TransferTestResource {
                            drop_event: "operation-drop",
                            events: &events,
                        })
                    }
                },
                TransferCallbacks {
                    start: |operation: &mut TransferTestResource<'_>| {
                        assert_eq!(operation.drop_event, "operation-drop");
                        events.borrow_mut().push("start");
                        i32::from(matches!(case, "immediate" | "immediate-error"))
                    },
                    result: |_: &mut TransferTestResource<'_>| {
                        events.borrow_mut().push("result");
                        if case == "immediate-error" {
                            Err(io::Error::other("completion refused"))
                        } else {
                            Ok(3)
                        }
                    },
                    error: || {
                        events.borrow_mut().push("error");
                        io::Error::from_raw_os_error(match case {
                            "failed-start" => 5,
                            "disconnected-start" => ERROR_BROKEN_PIPE.cast_signed(),
                            _ => ERROR_IO_PENDING.cast_signed(),
                        })
                    },
                    pending: || {
                        events.borrow_mut().push("pending");
                        TransferTestResource {
                            drop_event: "pending-drop",
                            events: &events,
                        }
                    },
                    closed: || {
                        events.borrow_mut().push("closed");
                        case == "closed"
                    },
                    cancel: |_: &mut TransferTestResource<'_>| {
                        assert!(!events.borrow().contains(&"pending-drop"));
                        events.borrow_mut().push("cancel");
                    },
                    wait: |_: &mut TransferTestResource<'_>, actual_timeout| {
                        assert_eq!(actual_timeout, timeout);
                        assert!(!events.borrow().contains(&"pending-drop"));
                        assert_ne!(case, "closed");
                        events.borrow_mut().push("wait");
                        match case {
                            "wait-disconnect" => Err(io::Error::from_raw_os_error(ERROR_BROKEN_PIPE.cast_signed())),
                            "wait-error" => Err(io::Error::from_raw_os_error(5)),
                            _ => Ok(7),
                        }
                    },
                },
            );
            assert_transfer_outcome(case, result, &events.borrow());
        }
    }

    fn assert_transfer_outcome(case: &str, result: io::Result<usize>, events: &[&str]) {
        match case {
            "create-error" => assert_eq!(result.unwrap_err().to_string(), "create refused"),
            "immediate" => assert_eq!(result.unwrap(), 3),
            "immediate-error" => assert_eq!(result.unwrap_err().to_string(), "completion refused"),
            "failed-start" | "wait-error" => assert_eq!(result.unwrap_err().raw_os_error(), Some(5)),
            "disconnected-start" | "wait-disconnect" => assert_eq!(result.unwrap(), 0),
            "closed" => assert_eq!(result.unwrap_err().kind(), io::ErrorKind::ConnectionAborted),
            _ => assert_eq!(result.unwrap(), 7),
        }
        let expected = match case {
            "create-error" => vec!["create"],
            "immediate" | "immediate-error" => vec!["create", "start", "result", "operation-drop"],
            "failed-start" | "disconnected-start" => vec!["create", "start", "error", "operation-drop"],
            "closed" => vec![
                "create",
                "start",
                "error",
                "pending",
                "closed",
                "cancel",
                "pending-drop",
                "operation-drop",
            ],
            _ => vec![
                "create",
                "start",
                "error",
                "pending",
                "closed",
                "wait",
                "pending-drop",
                "operation-drop",
            ],
        };
        assert_eq!(events, expected, "{case}");
    }

    #[test]
    fn november_peek_reports_exact_capacity_output_and_native_error_classes_without_io() {
        for case in [
            "empty",
            "handle-error",
            "closed",
            "other-error",
            "broken",
            "no-data",
            "not-connected",
            "ready",
        ] {
            let events = RefCell::new(Vec::new());
            let mut bytes = vec![0xa5; if case == "empty" { 0 } else { 3 }];
            let result = peek_pipe_with(
                &mut bytes,
                || {
                    events.borrow_mut().push("handle");
                    if case == "handle-error" {
                        Err(io::Error::other("handle refused"))
                    } else {
                        Ok(TransferTestResource {
                            drop_event: "handle-drop",
                            events: &events,
                        })
                    }
                },
                || {
                    events.borrow_mut().push("open");
                    if case == "closed" {
                        Err(io::ErrorKind::ConnectionAborted.into())
                    } else {
                        Ok(())
                    }
                },
                |_, bytes, capacity, read, available| {
                    events.borrow_mut().push("query");
                    assert_eq!(capacity, 3);
                    assert_eq!(bytes.len(), 3);
                    if case == "ready" {
                        bytes[..2].copy_from_slice(b"AB");
                        *read = 2;
                        *available = 2;
                        1
                    } else {
                        *read = u32::MAX;
                        *available = u32::MAX;
                        0
                    }
                },
                || {
                    assert_ne!(case, "ready");
                    events.borrow_mut().push("error");
                    io::Error::from_raw_os_error(match case {
                        "broken" => ERROR_BROKEN_PIPE.cast_signed(),
                        "no-data" => ERROR_NO_DATA.cast_signed(),
                        "not-connected" => ERROR_PIPE_NOT_CONNECTED.cast_signed(),
                        _ => 5,
                    })
                },
                || panic!("terminal peek outcomes must not inspect a deadline"),
                |_| panic!("terminal peek outcomes must not request sleeping"),
            );
            match case {
                "handle-error" => assert_eq!(result.unwrap_err().to_string(), "handle refused"),
                "closed" => assert_eq!(result.unwrap_err().kind(), io::ErrorKind::ConnectionAborted),
                "other-error" => assert_eq!(result.unwrap_err().raw_os_error(), Some(5)),
                "ready" => {
                    assert_eq!(result.unwrap(), 2);
                    assert_eq!(bytes, [b'A', b'B', 0xa5]);
                }
                _ => assert_eq!(result.unwrap(), 0),
            }
            assert_eq!(
                *events.borrow(),
                match case {
                    "empty" => vec![],
                    "handle-error" => vec!["handle"],
                    "closed" => vec!["handle", "open", "handle-drop"],
                    "ready" => vec!["handle", "open", "query", "handle-drop"],
                    _ => vec!["handle", "open", "query", "error", "handle-drop"],
                }
            );
        }
    }

    #[test]
    fn november_peek_deadline_uses_exact_retry_minimum_and_never_sleeps_after_expiration() {
        for remaining in [
            None,
            Some(Duration::ZERO),
            Some(Duration::from_micros(500)),
            Some(Duration::from_millis(5)),
        ] {
            let queries = Cell::new(0);
            let sleeps = RefCell::new(Vec::new());
            let mut bytes = [0; 3];
            let result = peek_pipe_with(
                &mut bytes,
                || Ok(()),
                || Ok(()),
                |(), bytes, capacity, read, available| {
                    assert_eq!(capacity, 3);
                    queries.set(queries.get() + 1);
                    if queries.get() == 1 {
                        *read = 0;
                        *available = 0;
                    } else {
                        assert_eq!(queries.get(), 2);
                        bytes.copy_from_slice(b"XYZ");
                        *read = 3;
                        *available = 7;
                    }
                    1
                },
                || panic!("successful peek must not fetch a native error"),
                || {
                    assert_eq!(queries.get(), 1);
                    remaining
                },
                |duration| sleeps.borrow_mut().push(duration),
            );
            if let Some(remaining) = remaining {
                assert_eq!(result.unwrap(), 3);
                assert_eq!(bytes, *b"XYZ");
                assert_eq!(queries.get(), 2);
                assert_eq!(
                    *sleeps.borrow(),
                    [if remaining < Duration::from_millis(2) {
                        remaining
                    } else {
                        Duration::from_millis(2)
                    }]
                );
            } else {
                let error = result.unwrap_err();
                assert_eq!(error.kind(), io::ErrorKind::TimedOut);
                assert_eq!(error.to_string(), "monitor pipe peek timed out");
                assert_eq!(queries.get(), 1);
                assert!(sleeps.borrow().is_empty());
                assert_eq!(bytes, [0; 3]);
            }
        }
    }

    #[test]
    fn november_pipe_buffer_length_clamps_only_above_the_native_u32_limit_without_allocation() {
        for length in [0_u32, 1, 4096, u32::MAX] {
            assert_eq!(pipe_buffer_length(length as usize), length);
        }
        #[cfg(target_pointer_width = "64")]
        {
            assert_eq!(pipe_buffer_length(u32::MAX as usize + 1), u32::MAX);
            assert_eq!(pipe_buffer_length(usize::MAX), u32::MAX);
        }
    }

    #[test]
    fn november_connect_retries_only_busy_pipes_with_exact_native_flags_and_bounded_sleeps() {
        let name = [116, 0];
        for remaining in [Duration::ZERO, Duration::from_micros(500), Duration::from_millis(5)] {
            let events = RefCell::new(Vec::new());
            let opens = Cell::new(0);
            let sleeps = RefCell::new(Vec::new());
            let handle = connect_pipe_with(
                &name,
                |actual_name, access, share, disposition, flags| {
                    events.borrow_mut().push("open");
                    assert_eq!(actual_name, name);
                    assert_eq!(access, 0xc000_0000);
                    assert_eq!(share, 0);
                    assert_eq!(disposition, 3);
                    assert_eq!(flags, 0x4011_0000);
                    opens.set(opens.get() + 1);
                    if opens.get() == 1 {
                        Err(io::Error::from_raw_os_error(ERROR_PIPE_BUSY.cast_signed()))
                    } else {
                        assert_eq!(opens.get(), 2);
                        Ok(TransferTestResource {
                            drop_event: "handle-drop",
                            events: &events,
                        })
                    }
                },
                || {
                    events.borrow_mut().push("remaining");
                    Some(remaining)
                },
                |duration| {
                    events.borrow_mut().push("sleep");
                    sleeps.borrow_mut().push(duration);
                },
            )
            .unwrap();
            assert_eq!(opens.get(), 2);
            assert_eq!(*events.borrow(), ["open", "remaining", "sleep", "open"]);
            assert_eq!(
                *sleeps.borrow(),
                [if remaining < Duration::from_millis(2) {
                    remaining
                } else {
                    Duration::from_millis(2)
                }]
            );
            drop(handle);
            assert_eq!(*events.borrow(), ["open", "remaining", "sleep", "open", "handle-drop"]);
        }
    }

    #[test]
    fn november_connect_returns_exact_nonbusy_errors_and_busy_deadline_expiration_without_sleep() {
        for case in ["other", "native", "busy"] {
            let opens = Cell::new(0);
            let deadlines = Cell::new(0);
            let error = connect_pipe_with::<()>(
                &[116, 0],
                |_, _, _, _, _| {
                    opens.set(opens.get() + 1);
                    assert_eq!(opens.get(), 1);
                    Err(match case {
                        "other" => io::Error::other("open refused"),
                        "native" => io::Error::from_raw_os_error(5),
                        _ => io::Error::from_raw_os_error(ERROR_PIPE_BUSY.cast_signed()),
                    })
                },
                || {
                    assert_eq!(case, "busy");
                    deadlines.set(deadlines.get() + 1);
                    None
                },
                |_| panic!("failed or expired connection attempts must not sleep"),
            )
            .unwrap_err();
            match case {
                "other" => assert_eq!(error.to_string(), "open refused"),
                "native" => assert_eq!(error.raw_os_error(), Some(5)),
                _ => {
                    assert_eq!(error.kind(), io::ErrorKind::TimedOut);
                    assert_eq!(error.to_string(), "monitor pipe connection timed out");
                }
            }
            assert_eq!(opens.get(), 1);
            assert_eq!(deadlines.get(), usize::from(case == "busy"));
        }
    }

    #[test]
    fn november_close_policy_preserves_cancel_disconnect_order_and_exact_native_errors() {
        for server in [false, true] {
            for cancel_code in [None, Some(ERROR_NOT_FOUND.cast_signed()), Some(5)] {
                for disconnect_code in [None, Some(ERROR_PIPE_NOT_CONNECTED.cast_signed()), Some(5)] {
                    let calls = RefCell::new(Vec::new());
                    let errors = RefCell::new(
                        [cancel_code, disconnect_code]
                            .into_iter()
                            .flatten()
                            .collect::<std::collections::VecDeque<_>>(),
                    );
                    let result = close_pipe_with(
                        server,
                        || {
                            calls.borrow_mut().push("cancel");
                            i32::from(cancel_code.is_none())
                        },
                        || {
                            calls.borrow_mut().push("disconnect");
                            i32::from(disconnect_code.is_none())
                        },
                        || io::Error::from_raw_os_error(errors.borrow_mut().pop_front().unwrap()),
                    );
                    let cancel_failed = cancel_code.is_some_and(|code| code != ERROR_NOT_FOUND.cast_signed());
                    let expected_error = if cancel_failed {
                        cancel_code
                    } else if server {
                        disconnect_code.filter(|code| *code != ERROR_PIPE_NOT_CONNECTED.cast_signed())
                    } else {
                        None
                    };
                    assert_eq!(result.err().and_then(|error| error.raw_os_error()), expected_error);
                    assert_eq!(
                        *calls.borrow(),
                        if server && !cancel_failed {
                            vec!["cancel", "disconnect"]
                        } else {
                            vec!["cancel"]
                        }
                    );
                }
            }
        }
    }

    #[test]
    fn november_operation_wait_uses_exact_deadlines_and_cancels_only_after_capturing_errors() {
        for (timeout, expected_wait) in [
            (None, INFINITE),
            (Some(Duration::ZERO), 1),
            (Some(Duration::from_nanos(1)), 1),
            (Some(Duration::from_millis(17)), 18),
        ] {
            for status in [WAIT_OBJECT_0, WAIT_TIMEOUT, u32::MAX] {
                let mut calls = Rc::new(RefCell::new(Vec::new()));
                let error_calls = Rc::clone(&calls);
                let result = wait_operation_with(
                    &mut calls,
                    timeout,
                    |calls, wait| {
                        assert_eq!(wait, expected_wait);
                        calls.borrow_mut().push("wait");
                        status
                    },
                    |calls| {
                        calls.borrow_mut().push("result");
                        Ok(23)
                    },
                    |calls| calls.borrow_mut().push("cancel"),
                    || {
                        error_calls.borrow_mut().push("last-error");
                        io::Error::from_raw_os_error(5)
                    },
                );
                match status {
                    WAIT_OBJECT_0 => {
                        assert_eq!(result.unwrap(), 23);
                        assert_eq!(*calls.borrow(), ["wait", "result"]);
                    }
                    WAIT_TIMEOUT => {
                        let error = result.unwrap_err();
                        assert_eq!(
                            (error.kind(), error.to_string()),
                            (io::ErrorKind::TimedOut, "monitor pipe I/O timed out".into())
                        );
                        assert_eq!(*calls.borrow(), ["wait", "cancel"]);
                    }
                    _ => {
                        assert_eq!(result.unwrap_err().raw_os_error(), Some(5));
                        assert_eq!(*calls.borrow(), ["wait", "last-error", "cancel"]);
                    }
                }
            }
        }
    }

    #[test]
    fn november_accept_completion_keeps_pending_until_success_and_forwards_nonblocking_waits() {
        for nonblocking in [false, true] {
            let mut context = 0;
            let mut pending = false;
            complete_accept_with(
                &mut pending,
                nonblocking,
                &mut context,
                |_, _| panic!("completed accept must not wait"),
                |_| panic!("completed accept must not fetch a result"),
                |_| panic!("completed accept has no error"),
            )
            .unwrap();
            for (status, completed) in [
                (WAIT_TIMEOUT, true),
                (u32::MAX, true),
                (WAIT_OBJECT_0, false),
                (WAIT_OBJECT_0, true),
            ] {
                let mut pending = true;
                let fetched = Cell::new(0);
                let result = complete_accept_with(
                    &mut pending,
                    nonblocking,
                    &mut context,
                    |_, wait| {
                        assert_eq!(wait, if nonblocking { 0 } else { INFINITE });
                        status
                    },
                    |_| {
                        fetched.set(fetched.get() + 1);
                        if completed { Ok(0) } else { Err(io::Error::from_raw_os_error(6)) }
                    },
                    |_| io::Error::from_raw_os_error(5),
                );
                assert_eq!(fetched.get(), usize::from(status == WAIT_OBJECT_0));
                if status == WAIT_OBJECT_0 && completed {
                    result.unwrap();
                    assert!(!pending);
                } else {
                    let error = result.unwrap_err();
                    if status == WAIT_TIMEOUT {
                        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
                    } else {
                        assert_eq!(error.raw_os_error(), Some(if status == WAIT_OBJECT_0 { 6 } else { 5 }));
                    }
                    assert!(pending);
                }
            }
        }
    }

    #[test]
    fn gamma_round2_native_connection_rejects_zero_timeout_with_exact_error_before_opening_a_pipe() {
        let descriptor = MonitorDescriptor {
            name: "in-memory only".into(),
            instance: None,
            process_id: 17,
            instance_id: crate::monitor::InstanceId::from_bytes([17; 16]),
            endpoint: MonitorEndpoint::NamedPipe(std::path::PathBuf::from(r"\\.\pipe\seismograph-memory-only")),
            owner: MonitorOwner::WindowsSid("S-1-5-18".into()),
        };
        let error = Connection::connect(&descriptor, Duration::ZERO).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(error.to_string(), "connection timeout must be positive and bounded");
    }

    #[test]
    fn gamma_round2_named_pipe_parameters_preserve_exact_native_flags_and_capacities() {
        assert_eq!(named_pipe_parameters(false), [0x4000_0003, 8, 255, 65_536, 65_536, 0]);
        assert_eq!(named_pipe_parameters(true), [0x4008_0003, 8, 255, 65_536, 65_536, 0]);
    }

    #[test]
    fn gamma_round2_accept_pending_distinguishes_immediate_connected_pending_and_os_error() {
        for connected in [-1, 1, 2] {
            assert!(!accept_pending(connected, || panic!("successful accept must not read last error")).unwrap());
        }
        assert!(!accept_pending(0, || io::Error::from_raw_os_error(535)).unwrap());
        assert!(accept_pending(0, || io::Error::from_raw_os_error(997)).unwrap());
        assert_eq!(
            accept_pending(0, || io::Error::from_raw_os_error(5)).unwrap_err().raw_os_error(),
            Some(5)
        );
    }

    #[test]
    fn gamma_round2_native_wait_milliseconds_preserve_rounding_and_saturating_fallback() {
        assert_eq!(operation_wait_millis(None), u32::MAX);
        for (duration, expected) in [
            (Duration::ZERO, 1),
            (Duration::from_micros(999), 1),
            (Duration::from_millis(1), 2),
            (Duration::from_millis(u64::from(u32::MAX) - 2), u32::MAX - 1),
            (Duration::from_millis(u64::from(u32::MAX) - 1), u32::MAX),
            (Duration::from_millis(u64::from(u32::MAX)), u32::MAX - 1),
            (Duration::MAX, u32::MAX - 1),
        ] {
            assert_eq!(operation_wait_millis(Some(duration)), expected);
        }
    }
    use super::*;

    #[test]
    fn gamma_survivor_native_transport_rejects_inputs_before_any_os_call() {
        let mut descriptor = MonitorDescriptor {
            name: "worker".into(),
            instance: None,
            process_id: 42,
            instance_id: crate::monitor::InstanceId::from_bytes([7; 16]),
            owner: crate::monitor::MonitorOwner::WindowsSid("S-1-5-21-1-2-3-1001".into()),
            endpoint: MonitorEndpoint::UnixSocket("unsupported".into()),
        };
        assert_eq!(
            Connection::connect(&descriptor, Duration::from_secs(1)).unwrap_err().to_string(),
            "Windows requires a local named pipe"
        );
        assert_eq!(
            Accept::new(&descriptor, true).unwrap_err().to_string(),
            "Windows requires a local named pipe"
        );
        assert_eq!(
            Listener::bind(&descriptor).unwrap_err().to_string(),
            "Windows requires a local named pipe"
        );
        descriptor.endpoint = MonitorEndpoint::NamedPipe("bad\0name".into());
        assert_eq!(
            Connection::connect(&descriptor, Duration::from_secs(1)).unwrap_err().to_string(),
            "native name contains a null character"
        );
        assert_eq!(
            Accept::new(&descriptor, true).unwrap_err().to_string(),
            "native name contains a null character"
        );
        descriptor.endpoint = MonitorEndpoint::NamedPipe(r"\\.\pipe\unused".into());
        descriptor.owner = crate::monitor::MonitorOwner::UnixUser(1);
        assert_eq!(
            Accept::new(&descriptor, true).unwrap_err().to_string(),
            "a Windows monitor requires a Windows user SID"
        );
        assert_eq!(
            check_timeout(Some(Duration::ZERO)).unwrap_err().to_string(),
            "monitor I/O timeout must be positive and bounded"
        );
    }

    #[test]
    fn gamma_survivor_native_disconnected_distinguishes_status_codes_without_handles() {
        for code in [ERROR_BROKEN_PIPE, ERROR_NO_DATA, ERROR_PIPE_NOT_CONNECTED] {
            assert!(disconnected(&io::Error::from_raw_os_error(code.cast_signed())));
        }
        for code in [5, 87, 997] {
            assert!(!disconnected(&io::Error::from_raw_os_error(code)));
        }
        assert!(!disconnected(&io::Error::new(io::ErrorKind::BrokenPipe, "not an OS status")));
    }

    #[test]
    fn gamma_survivor_native_empty_handle_connection_returns_errors_without_os_io() {
        let pipe = Arc::new(Pipe {
            handle: Mutex::new(None),
            server: false,
            closed: AtomicBool::new(false),
            pending_reads: super::super::PendingReads::default(),
        });
        let mut connection = Connection::new(pipe);
        assert_eq!(connection.peek(&mut [0]).unwrap_err().kind(), io::ErrorKind::ConnectionAborted);
        assert_eq!(connection.read(&mut [0]).unwrap_err().kind(), io::ErrorKind::ConnectionAborted);
        assert_eq!(connection.read(&mut []).unwrap(), 0);
        assert_eq!(connection.write(&[]).unwrap(), 0);
        assert_eq!(connection.peek(&mut []).unwrap(), 0);
        connection.close().unwrap();
        assert_eq!(connection.flush().unwrap_err().kind(), io::ErrorKind::ConnectionAborted);
    }

    #[test]
    fn disconnection_statuses_do_not_hide_other_native_errors() {
        for code in [ERROR_BROKEN_PIPE, ERROR_NO_DATA, ERROR_PIPE_NOT_CONNECTED] {
            assert!(disconnected(&io::Error::from_raw_os_error(code.cast_signed())));
        }
        assert!(!disconnected(&io::Error::from(io::ErrorKind::PermissionDenied)));
        assert!(!disconnected(&io::Error::other("native failure")));
    }

    #[test]
    #[cfg_attr(miri, ignore = "native pipe and event behavior is exercised natively")]
    fn native_connections_validate_identity_deadlines_and_empty_io() {
        let directory = crate::testing::Directory::new();
        let mut descriptor = directory.descriptor();
        let mut unsupported = descriptor.clone();
        unsupported.endpoint = MonitorEndpoint::UnixSocket("unsupported".into());
        assert_eq!(
            Connection::connect(&unsupported, Duration::from_secs(1)).unwrap_err().kind(),
            io::ErrorKind::Unsupported
        );
        for timeout in [Duration::ZERO, Duration::MAX] {
            assert_eq!(
                Connection::connect(&descriptor, timeout).unwrap_err().kind(),
                io::ErrorKind::InvalidInput
            );
        }
        {
            let _listener = Listener::bind(&descriptor).unwrap();
            let process_id = descriptor.process_id;
            descriptor.process_id = process_id.wrapping_add(1);
            assert_eq!(
                Connection::connect(&descriptor, Duration::from_secs(1)).unwrap_err().kind(),
                io::ErrorKind::PermissionDenied
            );
            descriptor.process_id = process_id;
        }
        let listener = Listener::bind(&descriptor).unwrap();
        listener.set_nonblocking(true).unwrap();
        assert!(listener.nonblocking.load(Ordering::Acquire));
        listener.set_nonblocking(false).unwrap();
        assert!(!listener.nonblocking.load(Ordering::Acquire));
        let mut client = Connection::connect(&descriptor, Duration::from_secs(1)).unwrap();
        let _server = listener.accept().unwrap();
        assert!(!client.read_is_pending());
        assert_eq!(client.write(b"ok").unwrap(), 2);
        assert_eq!((client.read(&mut []).unwrap(), client.write(&[]).unwrap()), (0, 0));
        for timeout in [Duration::ZERO, Duration::MAX] {
            assert_eq!(
                client.set_read_timeout(Some(timeout)).unwrap_err().kind(),
                io::ErrorKind::InvalidInput
            );
            assert_eq!(
                client.set_write_timeout(Some(timeout)).unwrap_err().kind(),
                io::ErrorKind::InvalidInput
            );
        }
        assert!(format!("{:?}", Operation::new().unwrap()).starts_with("Operation { event:"));
        client.close().unwrap();
        assert_eq!(client.ensure_open().unwrap_err().kind(), io::ErrorKind::ConnectionAborted);
        assert_eq!(client.flush().unwrap_err().kind(), io::ErrorKind::ConnectionAborted);
    }

    #[test]
    #[cfg_attr(miri, ignore = "native and careful suites verify pipe impersonation limits")]
    fn pipe_client_allows_identification_not_impersonation() {
        use windows_sys::Win32::Security::{
            GetTokenInformation, RevertToSelf, SecurityIdentification, TOKEN_QUERY, TokenImpersonationLevel,
        };
        use windows_sys::Win32::System::Pipes::ImpersonateNamedPipeClient;
        use windows_sys::Win32::System::Threading::{GetCurrentThread, OpenThreadToken};

        struct Revert;
        impl Drop for Revert {
            fn drop(&mut self) {
                // SAFETY: restores this test thread's original security context.
                assert_ne!(unsafe { RevertToSelf() }, 0);
            }
        }

        let directory = crate::testing::Directory::new();
        let descriptor = directory.descriptor();
        let listener = Listener::bind(&descriptor).unwrap();
        let mut client = Connection::connect(&descriptor, Duration::from_secs(1)).unwrap();
        let mut server = listener.accept().unwrap();
        client.write_all(&[1]).unwrap();
        server.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        server.read_exact(&mut [0]).unwrap();
        let handle = server.pipe.handle().unwrap();
        // SAFETY: the connected server pipe has read the client's data.
        assert_ne!(unsafe { ImpersonateNamedPipeClient(handle.as_raw_handle()) }, 0);
        let _revert = Revert;
        let mut token = ptr::null_mut();
        // SAFETY: GetCurrentThread returns a non-owning pseudo-handle.
        let thread = unsafe { GetCurrentThread() };
        // SAFETY: the current-thread pseudo-handle and token output are valid.
        assert_ne!(unsafe { OpenThreadToken(thread, TOKEN_QUERY, 1, &raw mut token) }, 0);
        let token = security::owned_handle(token).unwrap();
        let mut level: i32 = -1;
        let mut length = 0;
        assert_ne!(
            // SAFETY: level is a correctly sized writable impersonation-level buffer.
            unsafe {
                GetTokenInformation(
                    token.as_raw_handle(),
                    TokenImpersonationLevel,
                    ptr::from_mut(&mut level).cast(),
                    u32::try_from(std::mem::size_of_val(&level)).unwrap(),
                    &raw mut length,
                )
            },
            0
        );
        assert_eq!(level, SecurityIdentification);
    }
}
