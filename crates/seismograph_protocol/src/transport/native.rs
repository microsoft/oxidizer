// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt;
use std::io::{self, Read, Write};
use std::time::Duration;

use crate::monitor::MonitorDescriptor;
use crate::platform;

pub(super) enum Listener {
    System(platform::Listener),
    #[cfg(test)]
    Injected(crate::io::mock::Listener),
}

pub(super) enum Connection {
    System(platform::Connection),
    #[cfg(test)]
    Injected(crate::io::mock::Connection),
}

impl fmt::Debug for Listener {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::System(inner) => inner.fmt(f),
            #[cfg(test)]
            Self::Injected(inner) => inner.fmt(f),
        }
    }
}

impl fmt::Debug for Connection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::System(inner) => inner.fmt(f),
            #[cfg(test)]
            Self::Injected(inner) => inner.fmt(f),
        }
    }
}

impl Listener {
    pub(super) fn bind(descriptor: &MonitorDescriptor) -> io::Result<Self> {
        #[cfg(test)]
        if let Some(runtime) = TEST_NATIVE_RUNTIME.with_borrow(Clone::clone) {
            runtime.checkpoint(crate::io::IoOperation::Bind)?;
            return crate::io::mock::Listener::bind(&runtime, descriptor).map(Self::Injected);
        }
        platform::Listener::bind(descriptor).map(Self::System)
    }

    pub(super) fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        match self {
            Self::System(inner) => inner.set_nonblocking(nonblocking),
            #[cfg(test)]
            Self::Injected(inner) => inner.set_nonblocking(nonblocking),
        }
    }

    pub(super) fn accept(&self) -> io::Result<Connection> {
        match self {
            Self::System(inner) => inner.accept().map(Connection::System),
            #[cfg(test)]
            Self::Injected(inner) => {
                native_checkpoint(crate::io::IoOperation::Accept)?;
                inner.accept().map(Connection::Injected)
            }
        }
    }
}

impl Connection {
    pub(super) fn connect(descriptor: &MonitorDescriptor, timeout: Duration) -> io::Result<Self> {
        #[cfg(test)]
        if let Some(runtime) = TEST_NATIVE_RUNTIME.with_borrow(Clone::clone) {
            runtime.checkpoint(crate::io::IoOperation::Connect)?;
            return crate::io::mock::Connection::connect(&runtime, descriptor).map(Self::Injected);
        }
        platform::Connection::connect(descriptor, timeout).map(Self::System)
    }

    pub(super) fn try_clone(&self) -> io::Result<Self> {
        match self {
            Self::System(inner) => inner.try_clone().map(Self::System),
            #[cfg(test)]
            Self::Injected(inner) => {
                native_checkpoint(crate::io::IoOperation::Duplicate)?;
                inner.try_clone().map(Self::Injected)
            }
        }
    }

    pub(super) fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        match self {
            Self::System(inner) => inner.set_read_timeout(timeout),
            #[cfg(test)]
            Self::Injected(inner) => inner.set_read_timeout(timeout),
        }
    }

    pub(super) fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        match self {
            Self::System(inner) => inner.set_write_timeout(timeout),
            #[cfg(test)]
            Self::Injected(inner) => inner.set_write_timeout(timeout),
        }
    }

    pub(super) fn peek(&self, bytes: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::System(inner) => inner.peek(bytes),
            #[cfg(test)]
            Self::Injected(inner) => inner.peek(bytes),
        }
    }

    pub(super) fn close(&self) -> io::Result<()> {
        match self {
            Self::System(inner) => inner.close(),
            #[cfg(test)]
            Self::Injected(inner) => {
                inner.close();
                Ok(())
            }
        }
    }

    #[cfg(any(test, feature = "test-util"))]
    pub(super) fn read_is_pending(&self) -> bool {
        match self {
            Self::System(inner) => inner.read_is_pending(),
            #[cfg(test)]
            Self::Injected(_) => false,
        }
    }
}

impl Read for Connection {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::System(inner) => inner.read(buf),
            #[cfg(test)]
            Self::Injected(inner) => inner.read(buf),
        }
    }
}

impl Write for Connection {
    // #[gamma::skip(fn_value.ok, tag = "timeout", reason = "November 6 campaign 6030de79c6e0: returning Ok(1) without writing exceeded the timeout")]
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Self::System(inner) => inner.write(buf),
            #[cfg(test)]
            Self::Injected(inner) => inner.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::System(inner) => inner.flush(),
            #[cfg(test)]
            Self::Injected(inner) => inner.flush(),
        }
    }
}

#[cfg(any(test, feature = "test-util"))]
pub(super) fn use_mocked_io() -> bool {
    #[cfg(test)]
    {
        TEST_NATIVE_RUNTIME.with_borrow(Option::is_none)
    }
    #[cfg(not(test))]
    {
        true
    }
}

#[cfg(test)]
thread_local! {
    static TEST_NATIVE_RUNTIME: std::cell::RefCell<Option<crate::io::mock::Runtime>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
fn native_checkpoint(operation: crate::io::IoOperation) -> io::Result<()> {
    let runtime = TEST_NATIVE_RUNTIME
        .with_borrow(Clone::clone)
        .expect("an injected native owner is used within its owning backend scope");
    runtime.checkpoint(operation)
}

#[cfg(test)]
pub(super) fn with_in_memory_native_backend<R>(runtime: &crate::io::mock::Runtime, run: impl FnOnce() -> R) -> R {
    struct Reset(Option<crate::io::mock::Runtime>);
    impl Drop for Reset {
        fn drop(&mut self) {
            TEST_NATIVE_RUNTIME.set(self.0.take());
        }
    }
    let reset = Reset(TEST_NATIVE_RUNTIME.replace(Some(runtime.clone())));
    let result = run();
    drop(reset);
    result
}
