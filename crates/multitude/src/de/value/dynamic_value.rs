// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use allocator_api2::alloc::{Allocator, Global};

use super::{EnumValue, Map, Number};
use crate::Box;

/// An arena-owned dynamic Serde value.
///
/// ```
/// use multitude::de::{Number, Value};
///
/// let value: Value = Value::Number(Number::U8(3));
/// assert_eq!(value.as_number(), Some(&Number::U8(3)));
/// ```
#[derive(Debug, PartialEq)]
pub enum Value<A: Allocator + Clone = Global> {
    /// Unit, including a format's null token.
    Unit,
    /// An explicitly represented absent option.
    None,
    /// An explicitly represented present option.
    Some(Box<Value<A>, A>),
    /// A boolean.
    Bool(bool),
    /// A number.
    Number(Number),
    /// A character.
    Char(char),
    /// A UTF-8 string.
    String(Box<str, A>),
    /// A byte string.
    Bytes(Box<[u8], A>),
    /// An explicitly represented newtype value.
    Newtype(Box<Value<A>, A>),
    /// A sequence.
    Sequence(Box<[Value<A>], A>),
    /// An ordered map.
    Map(Map<A>),
    /// An explicitly represented externally tagged enum.
    Enum {
        /// The variant name.
        variant: Box<str, A>,
        /// The variant payload.
        value: EnumValue<A>,
    },
}
