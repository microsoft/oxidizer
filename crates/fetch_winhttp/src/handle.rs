// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::cell::Cell;
use std::ffi::c_void;
use std::fmt;
use std::marker::PhantomData;
use std::panic::RefUnwindSafe;
use std::ptr::NonNull;

use crate::bindings::{Bindings as _, Facade};

/// A non-null WinHTTP handle token that Rust treats only as an opaque value.
///
/// This type centralizes pointer validity and thread-safety assumptions without
/// granting ownership or closing the underlying WinHTTP handle.
#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) struct RawHandle(NonNull<c_void>);

impl RawHandle {
    pub(crate) const fn new(pointer: *mut c_void) -> Option<Self> {
        match NonNull::new(pointer) {
            Some(pointer) => Some(Self(pointer)),
            None => None,
        }
    }

    pub(crate) const fn as_ptr(self) -> *mut c_void {
        self.0.as_ptr()
    }

    pub(crate) const fn as_const_ptr(self) -> *const c_void {
        self.0.as_ptr().cast_const()
    }
}

impl fmt::Debug for RawHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Pointer::fmt(&self.as_ptr(), f)
    }
}

// SAFETY: RawHandle is an opaque WinHTTP token. Rust never dereferences the
// pointer, and WinHTTP permits handles to be passed between threads.
unsafe impl Send for RawHandle {}

// SAFETY: sharing the token does not grant unsynchronized Rust memory access;
// higher-level wrappers restrict which handle classes may be shared.
unsafe impl Sync for RawHandle {}

/// Owns a WinHTTP session handle and closes it when the transport session drops.
///
/// Session handles may be shared because WinHTTP serializes session-scoped
/// operations and the wrapper exposes no mutable Rust state.
#[derive(Debug)]
pub(crate) struct SessionHandle {
    raw: RawHandle,
    bindings: Facade,
}

impl SessionHandle {
    pub(crate) const fn new(raw: RawHandle, bindings: Facade) -> Self {
        Self { raw, bindings }
    }

    pub(crate) const fn raw(&self) -> RawHandle {
        self.raw
    }

    pub(crate) const fn bindings(&self) -> &Facade {
        &self.bindings
    }
}

impl Drop for SessionHandle {
    fn drop(&mut self) {
        let _ = self.bindings.close_handle(self.raw);
    }
}

/// Owns the logical WinHTTP connection handle used to open one request.
///
/// The wrapper is movable between threads but intentionally not `Sync`, which
/// prevents concurrent request creation through the same logical handle.
#[derive(Debug)]
pub(crate) struct ConnectHandle {
    raw: RawHandle,
    bindings: Facade,
    not_sync: PhantomData<Cell<()>>,
}

impl ConnectHandle {
    pub(crate) const fn new(raw: RawHandle, bindings: Facade) -> Self {
        Self {
            raw,
            bindings,
            not_sync: PhantomData,
        }
    }

    pub(crate) const fn raw(&self) -> RawHandle {
        self.raw
    }
}

impl Drop for ConnectHandle {
    fn drop(&mut self) {
        let _ = self.bindings.close_handle(self.raw);
    }
}

// `not_sync` is a marker-only `Cell` that stores no value or state. Sharing a
// reference across an unwind boundary therefore cannot expose a partial
// mutation, while the marker continues to keep the handle `!Sync`.
impl RefUnwindSafe for ConnectHandle {}

/// Owns one WinHTTP request handle for its complete asynchronous lifecycle.
///
/// The wrapper is movable between threads but intentionally not `Sync`; the
/// request driver serializes all operations issued through it.
#[derive(Debug)]
pub(crate) struct RequestHandle {
    raw: RawHandle,
    bindings: Facade,
    not_sync: PhantomData<Cell<()>>,
}

impl RequestHandle {
    pub(crate) const fn new(raw: RawHandle, bindings: Facade) -> Self {
        Self {
            raw,
            bindings,
            not_sync: PhantomData,
        }
    }

    pub(crate) const fn raw(&self) -> RawHandle {
        self.raw
    }

    pub(crate) const fn bindings(&self) -> &Facade {
        &self.bindings
    }
}

impl Drop for RequestHandle {
    fn drop(&mut self) {
        let _ = self.bindings.close_handle(self.raw);
    }
}

// `not_sync` is a marker-only `Cell` that stores no value or state. Sharing a
// reference across an unwind boundary therefore cannot expose a partial
// mutation, while the marker continues to keep the handle `!Sync`.
impl RefUnwindSafe for RequestHandle {}

#[cfg(test)]
mod tests {
    use std::ffi::c_void;
    use std::panic::{RefUnwindSafe, UnwindSafe};
    use std::sync::Arc;

    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::{ConnectHandle, RawHandle, RequestHandle, SessionHandle};
    use crate::bindings::{Facade, MockBindings};

    // This asserts the test-build enum, including its MockBindings variant.
    assert_impl_all!(Facade: UnwindSafe, RefUnwindSafe);
    assert_impl_all!(RawHandle: Send, Sync, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(SessionHandle: Send, Sync, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(ConnectHandle: Send, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(RequestHandle: Send, UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(ConnectHandle: Sync);
    assert_not_impl_any!(RequestHandle: Sync);

    #[test]
    fn each_wrapper_closes_its_handle_once() {
        let mut bindings = MockBindings::new();
        bindings.expect_close_handle().times(3).returning(|_| Ok(()));
        let facade = Facade::mock(Arc::new(bindings));

        drop(SessionHandle::new(raw_handle(1), facade.clone()));
        drop(ConnectHandle::new(raw_handle(2), facade.clone()));
        drop(RequestHandle::new(raw_handle(3), facade));
    }

    #[test]
    fn accessors_preserve_handle_and_facade() {
        let mut bindings = MockBindings::new();
        bindings.expect_close_handle().once().returning(|_| Ok(()));
        let facade = Facade::mock(Arc::new(bindings));
        let raw = raw_handle(1);
        let session = SessionHandle::new(raw, facade);

        assert_eq!(session.raw(), raw);
        assert!(matches!(session.bindings(), Facade::Mock(_)));
    }

    fn raw_handle(value: usize) -> RawHandle {
        RawHandle::new(value as *mut c_void).unwrap()
    }
}
