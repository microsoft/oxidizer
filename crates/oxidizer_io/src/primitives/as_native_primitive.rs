// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::Debug;

use crate::pal;

/// Marker trait for types that can be projected as a native primitive type.
///
/// Types marked with this trait will expose platform-specific methods like `as_socket()`,
/// depending on the target platform. You will need to use conditional compilation to
/// make use of such methods on the platforms where they exist.
pub trait AsNativePrimitive: AsNativePrimitivePrivate + Debug + Sized {}

pub trait AsNativePrimitivePrivate: Debug + Sized {
    fn as_pal_primitive(&self) -> &pal::PrimitiveFacade;
}

// If it implements AsNativePrimitivePrivate, it implements AsNativePrimitive too.
impl<T: AsNativePrimitivePrivate + Debug + Sized> AsNativePrimitive for T {}