// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::mem;

use derive_more::Display;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Networking::WinSock::SOCKET;

use crate::pal::windows::Bindings;
use crate::pal::{BindingsFacade, Primitive};
use crate::thread_safe::ThreadSafe;

#[derive(Clone, Debug, Display)]
#[display("{inner}")]
pub struct PrimitiveImpl {
    inner: NativePrimitive,

    bindings: BindingsFacade,
}

impl PrimitiveImpl {
    pub(crate) const fn from_handle(handle: HANDLE, bindings: BindingsFacade) -> Self {
        Self {
            // SAFETY: We are going to assume that all Windows API HANDLEs are thread-safe until
            // we receive evidence to the contrary. Seems to be true for all inspected HANDLES but
            // the type is so generic that who can say - it could be possible that some Windows API
            // one day says "you can only use this handle from the same thread"? But unlikely, as
            // the Windows API surface is generally fully thread-safe (even if not synchronized).
            inner: NativePrimitive::Handle(unsafe { ThreadSafe::new(handle) }),
            bindings,
        }
    }

    #[cfg_attr(test, mutants::skip)] // Socket logic not yet implemented, this is placeholder.
    pub(crate) const fn from_socket(socket: SOCKET, bindings: BindingsFacade) -> Self {
        Self {
            inner: NativePrimitive::Socket(socket),
            bindings,
        }
    }

    /// Returns the primitive as a HANDLE.
    ///
    /// We bind all primitives to I/O completion ports via HANDLE, so every supported primitive on
    /// Windows must be convertible to HANDLE.
    #[expect(clippy::transmute_ptr_to_ptr, reason = "TODO: provide rationale")]
    pub(crate) fn as_handle(&self) -> &HANDLE {
        match &self.inner {
            NativePrimitive::Handle(handle) => handle,
            // SAFETY: This is just how Windows rolls - a SOCKET is a specialization of HANDLE.
            // Internally, they are both a pointer-sized opaque value.
            NativePrimitive::Socket(socket) => unsafe {
                mem::transmute::<&SOCKET, &HANDLE>(socket)
            },
        }
    }

    #[cfg_attr(test, mutants::skip)] // Socket logic not yet implemented, this is placeholder.
    #[expect(clippy::match_wildcard_for_single_variants, reason = "Intentional")]
    pub(crate) const fn try_as_socket(&self) -> Option<&SOCKET> {
        match &self.inner {
            NativePrimitive::Socket(socket) => Some(socket),
            _ => None,
        }
    }
}

impl Primitive for PrimitiveImpl {
    fn close(&self) {
        // The type is a non-unique reference, so there is no purpose to dropping it or somehow
        // trying to limit this to happening only once. The caller is responsible for proper usage
        // and not calling `close` multiple times (primitives may be reused, so that would be bad!).
        //
        // In public APIs we ensure this by always using either `OwnedPrimitive` or
        // `&crate::Primitive`, the former of which does its own lifetime management and
        // the latter of which does not publicly expose any `close()` method.
        match self.inner {
            NativePrimitive::Handle(handle) => {
                // We ignore the error because there is not much we can do about it.
                // TODO: Some telemetry might be marginally useful here in case of errors.
                _ = self.bindings.close_handle(*handle);
            }
            NativePrimitive::Socket(socket) => {
                // We ignore the error because there is not much we can do about it.
                // TODO: Some telemetry might be marginally useful here in case of errors.
                _ = self.bindings.close_socket(socket);
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Display)]
enum NativePrimitive {
    #[display("{_0:?}")]
    Handle(ThreadSafe<HANDLE>),
    #[display("{_0:?}")]
    Socket(SOCKET),
}