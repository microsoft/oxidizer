// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#[cfg(test)]
use crate::pal::MockPrimitive;
use crate::pal::PrimitiveFacade;

/// An I/O primitive that has not yet been bound to the I/O subsystem. A native operating system
/// I/O primitive must be convertible to this type for it to be used with the I/O subsystem.
///
/// I/O primitives convertible into this type can be passed to [`Context::bind_primitive()`][1], at
/// which point they become bound to the I/O subsystem. Once bound, the I/O primitive is represented
/// by a [`BoundPrimitive`][2] which implements automatic resource management, releasing the
/// platform resources associated with the I/O primitive when dropped.
///
/// This type implements `From<T>` for all supported platform-specific primitives (e.g. `HANDLE`,
/// `SOCKET`, `FILE` and similar, depending on the build target platform).
///
/// [1]: crate::Context::bind_primitive
/// [2]: crate::BoundPrimitive
#[derive(Debug)]
pub struct UnboundPrimitive {
    pub(crate) pal_primitive: PrimitiveFacade,
}

impl UnboundPrimitive {
    pub(crate) const fn new(pal_primitive: PrimitiveFacade) -> Self {
        Self { pal_primitive }
    }

    /// Converts a mock primitive into an unbound primitive.
    #[cfg(test)]
    pub(crate) fn from_mock(mock: MockPrimitive) -> Self {
        Self {
            pal_primitive: mock.into(),
        }
    }
}