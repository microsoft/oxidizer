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

use crate::monitor::{MonitorDescriptor, MonitorEndpoint};

const RETRY_DELAY: Duration = Duration::from_millis(2);
// Large snapshots stream through bounded kernel buffers rather than a
// multi-megabyte allocation per client.
const PIPE_BUFFER_BYTES: u32 = 65_536;

#[derive(Debug)]
struct Pipe {
    handle: Mutex<Option<Arc<OwnedHandle>>>,
    server: bool,
    closed: AtomicBool,
    #[cfg(any(test, feature = "test-util"))]
    pending_reads: super::PendingReads,
}

impl Pipe {
    fn new(handle: OwnedHandle, server: bool) -> Self {
        Self {
            handle: Mutex::new(Some(Arc::new(handle))),
            server,
            closed: AtomicBool::new(false),
            #[cfg(any(test, feature = "test-util"))]
            pending_reads: super::PendingReads::default(),
        }
    }

    fn handle(&self) -> io::Result<Arc<OwnedHandle>> {
        self.handle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .cloned()
            .ok_or_else(|| io::ErrorKind::ConnectionAborted.into())
    }

    fn close(&self) -> io::Result<()> {
        self.closed.store(true, Ordering::Release);
        let Some(handle) = self.handle.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take() else {
            return Ok(());
        };
        // Only in-flight operations retain this handle after close. Once they
        // drain, clients release it even if Connection clones remain alive.
        // SAFETY: the owned handle remains live while pending operations drain.
        if unsafe { CancelIoEx(handle.as_raw_handle(), ptr::null()) } == 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(ERROR_NOT_FOUND.cast_signed()) {
                return Err(error);
            }
        }
        if self.server {
            // SAFETY: this is a live server-side named-pipe handle.
            if unsafe { DisconnectNamedPipe(handle.as_raw_handle()) } == 0 {
                let error = io::Error::last_os_error();
                if error.raw_os_error() != Some(ERROR_PIPE_NOT_CONNECTED.cast_signed()) {
                    return Err(error);
                }
            }
        }
        Ok(())
    }
}

struct Operation {
    overlapped: Box<OVERLAPPED>,
    event: OwnedHandle,
}

impl std::fmt::Debug for Operation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Operation").field("event", &self.event).finish_non_exhaustive()
    }
}

#[expect(
    clippy::non_send_fields_in_send_ty,
    reason = "OVERLAPPED has a stable owned address and is exclusively accessed; pending kernel operations are drained before moving or freeing it"
)]
// SAFETY: OVERLAPPED points only to its owned event and has a stable Box address.
// Access is exclusive or protected by the listener mutex; pending operations
// are cancelled and drained before either field is freed.
unsafe impl Send for Operation {}

impl Operation {
    fn new() -> io::Result<Self> {
        // SAFETY: an unnamed non-inheritable event needs no external pointers.
        let event = security::owned_handle(unsafe { CreateEventW(ptr::null(), 1, 0, ptr::null()) })?;
        let overlapped = Box::new(OVERLAPPED {
            hEvent: event.as_raw_handle(),
            ..OVERLAPPED::default()
        });
        Ok(Self { overlapped, event })
    }

    fn pointer(&mut self) -> *mut OVERLAPPED {
        ptr::from_mut(self.overlapped.as_mut())
    }

    fn cancel_and_drain(&mut self, handle: HANDLE) {
        // SAFETY: handle and OVERLAPPED remain live until completion is drained.
        unsafe { CancelIoEx(handle, self.pointer()) };
        let mut transferred = 0;
        // SAFETY: waiting drains all kernel access before OVERLAPPED or buffers are freed.
        unsafe { GetOverlappedResult(handle, self.pointer(), &raw mut transferred, 1) };
    }

    fn result(&mut self, handle: HANDLE) -> io::Result<usize> {
        let mut transferred = 0;
        // SAFETY: the completed operation and handle are still live.
        if unsafe { GetOverlappedResult(handle, self.pointer(), &raw mut transferred, 0) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(transferred as usize)
    }

    fn wait(&mut self, handle: HANDLE, timeout: Option<Duration>) -> io::Result<usize> {
        let wait = timeout.map_or(INFINITE, |duration| {
            u32::try_from(duration.as_millis().saturating_add(1)).unwrap_or(INFINITE - 1)
        });
        // SAFETY: the event is owned and live throughout the wait.
        match unsafe { WaitForSingleObject(self.event.as_raw_handle(), wait) } {
            WAIT_OBJECT_0 => self.result(handle),
            WAIT_TIMEOUT => {
                self.cancel_and_drain(handle);
                Err(io::Error::new(io::ErrorKind::TimedOut, "monitor pipe I/O timed out"))
            }
            _ => {
                let error = io::Error::last_os_error();
                self.cancel_and_drain(handle);
                Err(error)
            }
        }
    }
}

#[derive(Debug)]
struct Accept {
    pipe: Arc<Pipe>,
    handle: Arc<OwnedHandle>,
    operation: Operation,
    pending: bool,
}

impl Accept {
    fn new(descriptor: &MonitorDescriptor, first: bool) -> io::Result<Self> {
        let MonitorEndpoint::NamedPipe(path) = &descriptor.endpoint else {
            return Err(io::Error::new(io::ErrorKind::Unsupported, "Windows requires a local named pipe"));
        };
        let name = security::wide(path.as_os_str())?;
        let attributes = security::Attributes::for_owner(&descriptor.owner)?;
        let flags = PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED | if first { FILE_FLAG_FIRST_PIPE_INSTANCE } else { 0 };
        // SAFETY: name and security descriptor are valid for the creation call.
        let handle = unsafe {
            CreateNamedPipeW(
                name.as_ptr(),
                flags,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                PIPE_BUFFER_BYTES,
                PIPE_BUFFER_BYTES,
                0,
                &attributes.native(),
            )
        };
        let pipe = Arc::new(Pipe::new(security::owned_handle(handle)?, true));
        let handle = pipe.handle()?;
        let mut operation = Operation::new()?;
        // SAFETY: OVERLAPPED has a stable address and outlives any pending accept.
        let connected = unsafe { ConnectNamedPipe(handle.as_raw_handle(), operation.pointer()) };
        let pending = if connected != 0 {
            false
        } else {
            let error = io::Error::last_os_error();
            match error.raw_os_error() {
                Some(code) if code == ERROR_PIPE_CONNECTED.cast_signed() => false,
                Some(code) if code == ERROR_IO_PENDING.cast_signed() => true,
                _ => return Err(error),
            }
        };
        Ok(Self {
            pipe,
            handle,
            operation,
            pending,
        })
    }

    fn complete(&mut self, nonblocking: bool) -> io::Result<()> {
        if !self.pending {
            return Ok(());
        }
        // SAFETY: the pending accept owns its event and OVERLAPPED.
        match unsafe { WaitForSingleObject(self.operation.event.as_raw_handle(), if nonblocking { 0 } else { INFINITE }) } {
            WAIT_TIMEOUT => return Err(io::ErrorKind::WouldBlock.into()),
            WAIT_OBJECT_0 => {}
            _ => return Err(io::Error::last_os_error()),
        }
        self.operation.result(self.handle.as_raw_handle())?;
        self.pending = false;
        Ok(())
    }
}

impl Drop for Accept {
    fn drop(&mut self) {
        if self.pending {
            self.operation.cancel_and_drain(self.handle.as_raw_handle());
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
        Ok(Self {
            descriptor: descriptor.clone(),
            accept: Mutex::new(Accept::new(descriptor, true)?),
            nonblocking: AtomicBool::new(false),
        })
    }

    #[expect(
        clippy::unnecessary_wraps,
        reason = "shared listener API also covers fallible Unix socket configuration"
    )]
    pub(crate) fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        self.nonblocking.store(nonblocking, Ordering::Release);
        Ok(())
    }

    pub(crate) fn accept(&self) -> io::Result<Connection> {
        let mut accept = self.accept.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Err(error) = accept.complete(self.nonblocking.load(Ordering::Acquire)) {
            if disconnected(&error) {
                let next = Accept::new(&self.descriptor, false)?;
                *accept = next;
                return Err(io::ErrorKind::WouldBlock.into());
            }
            return Err(error);
        }
        // Create the next instance while the original remains open, so there is
        // never a gap in which another process can take the pipe name.
        let next = Accept::new(&self.descriptor, false)?;
        let connected = std::mem::replace(&mut *accept, next);
        Ok(Connection::new(Arc::clone(&connected.pipe)))
    }
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
            .filter(|deadline| *deadline > Instant::now())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "connection timeout must be positive and bounded"))?;
        let handle = loop {
            // Identification permits the server to inspect the client identity,
            // never to borrow an elevated client's privileges.
            // SAFETY: name is terminated; the returned handle is non-inheritable.
            let handle = unsafe {
                CreateFileW(
                    name.as_ptr(),
                    GENERIC_READ | GENERIC_WRITE,
                    0,
                    ptr::null(),
                    OPEN_EXISTING,
                    FILE_FLAG_OVERLAPPED | SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
                    ptr::null_mut(),
                )
            };
            match security::owned_handle(handle) {
                Ok(handle) => break handle,
                Err(error) if error.raw_os_error() == Some(ERROR_PIPE_BUSY.cast_signed()) => {
                    let remaining = deadline
                        .checked_duration_since(Instant::now())
                        .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "monitor pipe connection timed out"))?;
                    std::thread::sleep(remaining.min(RETRY_DELAY));
                }
                Err(error) => return Err(error),
            }
        };
        let mut process_id = 0;
        // SAFETY: handle is a live connected pipe and process_id is writable.
        if unsafe { GetNamedPipeServerProcessId(handle.as_raw_handle(), &raw mut process_id) } == 0 {
            return Err(io::Error::last_os_error());
        }
        if process_id != descriptor.process_id || security::handle_owner(handle.as_raw_handle())? != descriptor.owner {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "connected monitor server has a different OS identity",
            ));
        }
        Ok(Self::new(Arc::new(Pipe::new(handle, false))))
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

    pub(crate) fn peek(&self, bytes: &mut [u8]) -> io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        let timeout = *self.read_timeout.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let deadline = timeout.and_then(|timeout| Instant::now().checked_add(timeout));
        let handle = self.pipe.handle()?;
        loop {
            self.ensure_open()?;
            let mut read = 0;
            let mut available = 0;
            // SAFETY: pipe is live and all buffers/count pointers are valid.
            let result = unsafe {
                PeekNamedPipe(
                    handle.as_raw_handle(),
                    bytes.as_mut_ptr().cast(),
                    u32::try_from(bytes.len()).unwrap_or(u32::MAX),
                    &raw mut read,
                    &raw mut available,
                    ptr::null_mut(),
                )
            };
            if result == 0 {
                let error = io::Error::last_os_error();
                return if disconnected(&error) { Ok(0) } else { Err(error) };
            }
            if available != 0 {
                return Ok(read as usize);
            }
            let remaining = match deadline {
                Some(deadline) => deadline
                    .checked_duration_since(Instant::now())
                    .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "monitor pipe peek timed out"))?,
                None => RETRY_DELAY,
            };
            std::thread::sleep(remaining.min(RETRY_DELAY));
        }
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

    fn perform(
        &self,
        direction: Direction,
        timeout: Option<Duration>,
        start: impl FnOnce(HANDLE, *mut OVERLAPPED) -> i32,
    ) -> io::Result<usize> {
        self.ensure_open()?;
        #[cfg(not(any(test, feature = "test-util")))]
        let _ = direction;
        let handle = self.pipe.handle()?;
        let mut operation = Operation::new()?;
        if start(handle.as_raw_handle(), operation.pointer()) != 0 {
            return operation.result(handle.as_raw_handle());
        }
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(ERROR_IO_PENDING.cast_signed()) {
            return if disconnected(&error) { Ok(0) } else { Err(error) };
        }
        #[cfg(any(test, feature = "test-util"))]
        let _pending = matches!(direction, Direction::Read).then(|| self.pipe.pending_reads.enter());
        // Close may have raced with starting the operation; cancel again after
        // it is registered so a clone cannot remain blocked indefinitely.
        if self.pipe.closed.load(Ordering::Acquire) {
            operation.cancel_and_drain(handle.as_raw_handle());
            return Err(io::ErrorKind::ConnectionAborted.into());
        }
        operation
            .wait(handle.as_raw_handle(), timeout)
            .or_else(|error| if disconnected(&error) { Ok(0) } else { Err(error) })
    }
}

impl Read for Connection {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let timeout = *self.read_timeout.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        self.perform(Direction::Read, timeout, |handle, overlapped| {
            // SAFETY: buf and OVERLAPPED remain live until perform drains the
            // operation, including on cancellation or a deadline.
            unsafe {
                ReadFile(
                    handle,
                    buf.as_mut_ptr(),
                    u32::try_from(buf.len()).unwrap_or(u32::MAX),
                    ptr::null_mut(),
                    overlapped,
                )
            }
        })
    }
}

impl Write for Connection {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let timeout = *self.write_timeout.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        self.perform(Direction::Write, timeout, |handle, overlapped| {
            // SAFETY: buf and OVERLAPPED remain live until perform drains the operation.
            unsafe {
                WriteFile(
                    handle,
                    buf.as_ptr(),
                    u32::try_from(buf.len()).unwrap_or(u32::MAX),
                    ptr::null_mut(),
                    overlapped,
                )
            }
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
    use super::*;

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
