// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Synchronous local byte streams with OS authorization and cancellable I/O.

use std::io::{self, Read, Write};
use std::time::Duration;

use crate::io::{IoOperation, MonitorIo};
use crate::monitor::MonitorDescriptor;
use crate::platform;

/// Local monitor listener. Dropping it removes its own Unix socket, or closes
/// the pending Windows pipe instance.
#[derive(Debug)]
pub struct Listener {
    inner: ListenerInner,
    io: MonitorIo,
    descriptor: MonitorDescriptor,
    #[cfg(any(test, feature = "test-util"))]
    pub(crate) fixture: Option<crate::testing::Directory>,
}

#[derive(Debug)]
enum ListenerInner {
    Native(platform::Listener),
    #[cfg(any(test, feature = "test-util"))]
    Mocked(crate::io::mock::Listener),
}

impl Listener {
    /// Binds a new endpoint owned by the current user.
    ///
    /// # Errors
    ///
    /// Fails for an unsupported endpoint, invalid owner, occupied name, or
    /// native listener/access-control failure.
    pub fn bind(io: &MonitorIo, descriptor: &MonitorDescriptor) -> io::Result<Self> {
        if descriptor.owner != io.current_owner()? || !descriptor.endpoint_matches_identity() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "monitor endpoint owner or identity does not match",
            ));
        }
        io.checkpoint(IoOperation::Bind)?;
        #[cfg(any(test, feature = "test-util"))]
        let inner = match io.mocked() {
            Some(runtime) => ListenerInner::Mocked(crate::io::mock::Listener::bind(runtime, descriptor)?),
            None => ListenerInner::Native(platform::Listener::bind(descriptor)?),
        };
        #[cfg(not(any(test, feature = "test-util")))]
        let inner = ListenerInner::Native(platform::Listener::bind(descriptor)?);
        Ok(Self {
            inner,
            io: io.clone(),
            descriptor: descriptor.clone(),
            #[cfg(any(test, feature = "test-util"))]
            fixture: None,
        })
    }

    /// Returns the descriptor for this listener.
    #[must_use]
    pub const fn descriptor(&self) -> &MonitorDescriptor {
        &self.descriptor
    }

    /// Controls whether accepting waits for a client.
    ///
    /// # Errors
    ///
    /// Returns an error if the native listener cannot be configured.
    pub fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        self.io.checkpoint(IoOperation::ConfigureListener)?;
        match &self.inner {
            ListenerInner::Native(inner) => inner.set_nonblocking(nonblocking),
            #[cfg(any(test, feature = "test-util"))]
            ListenerInner::Mocked(inner) => inner.set_nonblocking(nonblocking),
        }
    }

    /// Accepts an OS-authorized connection.
    ///
    /// # Errors
    ///
    /// Returns `WouldBlock` for an idle nonblocking listener, `PermissionDenied`
    /// for an unauthorized Unix peer, or the native I/O error.
    pub fn accept(&self) -> io::Result<Connection> {
        self.io.checkpoint(IoOperation::Accept)?;
        let inner = match &self.inner {
            ListenerInner::Native(inner) => ConnectionInner::Native(inner.accept()?),
            #[cfg(any(test, feature = "test-util"))]
            ListenerInner::Mocked(inner) => ConnectionInner::Mocked(inner.accept()?),
        };
        Ok(Connection {
            inner,
            io: self.io.clone(),
            #[cfg(any(test, feature = "test-util"))]
            observations: std::sync::Arc::default(),
        })
    }
}

/// An OS-authorized local byte stream. Clones share the underlying connection;
/// [`close`](Self::close) cancels pending I/O on all clones.
#[derive(Debug)]
pub struct Connection {
    inner: ConnectionInner,
    io: MonitorIo,
    #[cfg(any(test, feature = "test-util"))]
    observations: std::sync::Arc<ConnectionObservations>,
}

#[cfg(any(test, feature = "test-util"))]
#[derive(Debug, Default)]
struct ConnectionObservations {
    bytes_read: std::sync::atomic::AtomicUsize,
    close_calls: std::sync::atomic::AtomicUsize,
}

#[derive(Debug)]
enum ConnectionInner {
    Native(platform::Connection),
    #[cfg(any(test, feature = "test-util"))]
    Mocked(crate::io::mock::Connection),
}

impl Connection {
    #[cfg(any(test, feature = "test-util"))]
    pub(crate) fn bytes_read(&self) -> usize {
        self.observations.bytes_read.load(std::sync::atomic::Ordering::Acquire)
    }

    #[cfg(any(test, feature = "test-util"))]
    pub(crate) fn close_calls(&self) -> usize {
        self.observations.close_calls.load(std::sync::atomic::Ordering::Acquire)
    }

    #[cfg(any(test, feature = "test-util"))]
    pub(crate) fn read_is_pending(&self) -> io::Result<bool> {
        match &self.inner {
            ConnectionInner::Native(inner) => Ok(inner.read_is_pending()),
            ConnectionInner::Mocked(_) => Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "pending native reads require a native connection",
            )),
        }
    }

    /// Connects and checks the server's OS identity against its descriptor.
    ///
    /// Unix kernels without atomic socket close-on-exec, including Apple
    /// platforms, require callers using raw fork/exec to serialize process
    /// spawning with connection creation.
    ///
    /// # Errors
    ///
    /// Fails when access is denied, the server identity differs, the connection
    /// deadline expires, or the native connection fails.
    pub fn connect(io: &MonitorIo, descriptor: &MonitorDescriptor, timeout: Duration) -> io::Result<Self> {
        if !descriptor.endpoint_matches_identity() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "monitor endpoint does not match its identity",
            ));
        }
        if timeout.is_zero() || io.instant().checked_add(timeout).is_none() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "connection timeout must be positive and bounded",
            ));
        }
        io.checkpoint(IoOperation::Connect)?;
        #[cfg(any(test, feature = "test-util"))]
        let inner = match io.mocked() {
            Some(runtime) => ConnectionInner::Mocked(crate::io::mock::Connection::connect(runtime, descriptor)?),
            None => ConnectionInner::Native(platform::Connection::connect(descriptor, timeout)?),
        };
        #[cfg(not(any(test, feature = "test-util")))]
        let inner = ConnectionInner::Native(platform::Connection::connect(descriptor, timeout)?);
        Ok(Self {
            inner,
            io: io.clone(),
            #[cfg(any(test, feature = "test-util"))]
            observations: std::sync::Arc::default(),
        })
    }

    #[cfg(any(test, feature = "test-util"))]
    pub(crate) fn from_mocked(io: MonitorIo, connection: crate::io::mock::Connection) -> Self {
        Self {
            io,
            inner: ConnectionInner::Mocked(connection),
            observations: std::sync::Arc::new(ConnectionObservations::default()),
        }
    }

    /// Duplicates the connection for cancellation tracking.
    ///
    /// Timeout sharing between clones is platform-dependent. Configure
    /// deadlines on the clone that will perform I/O.
    ///
    /// # Errors
    ///
    /// Returns the native handle-duplication error.
    pub fn try_clone(&self) -> io::Result<Self> {
        self.io.checkpoint(IoOperation::Duplicate)?;
        let inner = match &self.inner {
            ConnectionInner::Native(inner) => ConnectionInner::Native(inner.try_clone()?),
            #[cfg(any(test, feature = "test-util"))]
            ConnectionInner::Mocked(inner) => ConnectionInner::Mocked(inner.try_clone()?),
        };
        Ok(Self {
            inner,
            io: self.io.clone(),
            #[cfg(any(test, feature = "test-util"))]
            observations: std::sync::Arc::clone(&self.observations),
        })
    }

    /// Sets a deadline for each subsequent read or peek.
    ///
    /// # Errors
    ///
    /// Fails for a zero timeout or native configuration error.
    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.io.checkpoint(IoOperation::ReadTimeout)?;
        match &self.inner {
            ConnectionInner::Native(inner) => inner.set_read_timeout(timeout),
            #[cfg(any(test, feature = "test-util"))]
            ConnectionInner::Mocked(inner) => inner.set_read_timeout(timeout),
        }
    }

    /// Sets a deadline for each subsequent write.
    ///
    /// # Errors
    ///
    /// Fails for a zero timeout or native configuration error.
    pub fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.io.checkpoint(IoOperation::WriteTimeout)?;
        match &self.inner {
            ConnectionInner::Native(inner) => inner.set_write_timeout(timeout),
            #[cfg(any(test, feature = "test-util"))]
            ConnectionInner::Mocked(inner) => inner.set_write_timeout(timeout),
        }
    }

    /// Reads without consuming bytes. Zero means the peer closed cleanly.
    ///
    /// # Errors
    ///
    /// Returns the read timeout, cancellation, or native I/O error.
    pub fn peek(&self, bytes: &mut [u8]) -> io::Result<usize> {
        self.io.checkpoint(IoOperation::Peek)?;
        match &self.inner {
            ConnectionInner::Native(inner) => inner.peek(bytes),
            #[cfg(any(test, feature = "test-util"))]
            ConnectionInner::Mocked(inner) => inner.peek(bytes),
        }
    }

    /// Closes both directions and cancels pending I/O.
    ///
    /// Named pipes do not support socket-style half-closes.
    ///
    /// # Errors
    ///
    /// Returns a native cancellation or shutdown error.
    pub fn close(&self) -> io::Result<()> {
        #[cfg(any(test, feature = "test-util"))]
        self.observations.close_calls.fetch_add(1, std::sync::atomic::Ordering::Release);
        let injected = self.io.checkpoint(IoOperation::Close);
        let closed = match &self.inner {
            ConnectionInner::Native(inner) => inner.close(),
            #[cfg(any(test, feature = "test-util"))]
            ConnectionInner::Mocked(inner) => {
                inner.close();
                Ok(())
            }
        };
        injected.and(closed)
    }
}

impl Read for Connection {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.io.checkpoint(IoOperation::Read)?;
        let result = match &mut self.inner {
            ConnectionInner::Native(inner) => inner.read(buf),
            #[cfg(any(test, feature = "test-util"))]
            ConnectionInner::Mocked(inner) => inner.read(buf),
        };
        #[cfg(any(test, feature = "test-util"))]
        if let Ok(count) = result {
            self.observations.bytes_read.fetch_add(count, std::sync::atomic::Ordering::Release);
        }
        result
    }
}

impl Write for Connection {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.io.checkpoint(IoOperation::Write)?;
        match &mut self.inner {
            ConnectionInner::Native(inner) => inner.write(buf),
            #[cfg(any(test, feature = "test-util"))]
            ConnectionInner::Mocked(inner) => inner.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        self.io.checkpoint(IoOperation::Flush)?;
        match &mut self.inner {
            ConnectionInner::Native(inner) => inner.flush(),
            #[cfg(any(test, feature = "test-util"))]
            ConnectionInner::Mocked(inner) => inner.flush(),
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn configuration_and_write_calls_preserve_injected_failures() {
        let (io, control) = MonitorIo::new_mocked();
        let directory = crate::discovery::Directory::current(&io).unwrap();
        let owner = directory.owner().clone();
        let instance_id = crate::monitor::InstanceId::from_bytes([2; 16]);
        let descriptor = MonitorDescriptor {
            name: "worker".into(),
            instance: None,
            process_id: io.process_id(),
            endpoint: crate::monitor::MonitorEndpoint::for_instance(directory.path(), &owner, io.process_id(), instance_id),
            instance_id,
            owner,
        };
        let listener = Listener::bind(&io, &descriptor).unwrap();
        control.fail_next(IoOperation::ConfigureListener, io::ErrorKind::PermissionDenied.into());
        assert_eq!(listener.set_nonblocking(true).unwrap_err().kind(), io::ErrorKind::PermissionDenied);
        let (mut connection, _peer) = control.connected_pair();
        control.fail_next(IoOperation::ReadTimeout, io::ErrorKind::PermissionDenied.into());
        assert_eq!(
            connection.set_read_timeout(Some(Duration::from_secs(1))).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        control.fail_next(IoOperation::WriteTimeout, io::ErrorKind::PermissionDenied.into());
        assert_eq!(
            connection.set_write_timeout(Some(Duration::from_secs(1))).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        control.fail_next(IoOperation::Write, io::ErrorKind::PermissionDenied.into());
        assert_eq!(connection.write(b"bytes").unwrap_err().kind(), io::ErrorKind::PermissionDenied);
    }

    #[test]
    fn connections_reject_zero_and_overflowing_deadlines_before_opening() {
        let (io, _) = MonitorIo::new_mocked();
        let directory = crate::discovery::Directory::current(&io).unwrap();
        let owner = directory.owner().clone();
        let instance_id = crate::monitor::InstanceId::from_bytes([1; 16]);
        let descriptor = MonitorDescriptor {
            name: "worker".into(),
            instance: None,
            process_id: 42,
            endpoint: crate::monitor::MonitorEndpoint::for_instance(directory.path(), &owner, 42, instance_id),
            instance_id,
            owner,
        };
        for timeout in [Duration::ZERO, Duration::MAX] {
            assert_eq!(
                Connection::connect(&io, &descriptor, timeout).unwrap_err().kind(),
                io::ErrorKind::InvalidInput
            );
        }
    }
}
