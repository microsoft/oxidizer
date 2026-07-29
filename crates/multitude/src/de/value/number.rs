// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// A number, retaining the exact category and width supplied by Serde.
///
/// ```
/// use multitude::de::Number;
///
/// let number = Number::U16(512);
/// assert_eq!(number, Number::U16(512));
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Number {
    /// An 8-bit signed integer.
    I8(i8),
    /// A 16-bit signed integer.
    I16(i16),
    /// A 32-bit signed integer.
    I32(i32),
    /// A 64-bit signed integer.
    I64(i64),
    /// A 128-bit signed integer.
    I128(i128),
    /// An 8-bit unsigned integer.
    U8(u8),
    /// A 16-bit unsigned integer.
    U16(u16),
    /// A 32-bit unsigned integer.
    U32(u32),
    /// A 64-bit unsigned integer.
    U64(u64),
    /// A 128-bit unsigned integer.
    U128(u128),
    /// A 32-bit floating-point number.
    F32(f32),
    /// A 64-bit floating-point number.
    F64(f64),
}
