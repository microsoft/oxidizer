// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use windows::Win32::Foundation::HANDLE;
use windows::Win32::Networking::WinSock::SOCKET;

use crate::UnboundPrimitive;
use crate::pal::{BindingsFacade, PrimitiveImpl};

impl UnboundPrimitive {
    /// (Windows only) Creates a new [`UnboundPrimitive`] from a Windows `HANDLE`.
    ///
    /// A Windows `HANDLE` is a general purpose data type. The caller is responsible for
    /// ensuring that the `HANDLE` is of a type on which I/O operations are meaningful to
    /// execute.
    #[must_use]
    pub fn from_handle(handle: HANDLE) -> Self {
        let pal_primitive = PrimitiveImpl::from_handle(handle, BindingsFacade::real());
        Self::new(pal_primitive.into())
    }

    /// (Windows only) Creates a new [`UnboundPrimitive`] from a Windows `SOCKET`.
    #[must_use]
    pub fn from_socket(socket: SOCKET) -> Self {
        let pal_primitive = PrimitiveImpl::from_socket(socket, BindingsFacade::real());
        Self::new(pal_primitive.into())
    }
}

impl From<HANDLE> for UnboundPrimitive {
    fn from(handle: HANDLE) -> Self {
        Self::from_handle(handle)
    }
}

impl From<SOCKET> for UnboundPrimitive {
    fn from(socket: SOCKET) -> Self {
        Self::from_socket(socket)
    }
}