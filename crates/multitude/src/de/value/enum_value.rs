// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use allocator_api2::alloc::{Allocator, Global};

use super::{Map, Value};
use crate::Box;

/// The payload of an explicitly represented enum.
///
/// ```
/// use multitude::de::EnumValue;
///
/// let payload: EnumValue = EnumValue::Unit;
/// assert!(matches!(payload, EnumValue::Unit));
/// ```
#[derive(Debug, PartialEq)]
pub enum EnumValue<A: Allocator + Clone = Global> {
    /// A unit variant.
    Unit,
    /// A newtype variant.
    Newtype(Box<Value<A>, A>),
    /// A tuple variant.
    Tuple(Box<[Value<A>], A>),
    /// A struct variant.
    Struct(Map<A>),
}
