// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Field-coercion helpers invoked by `#[resolver]`-generated code.
//!
//! Static helpers coerce raw captures; dynamic helpers retrieve captures by
//! index before coercion.

use alloc::borrow::Cow;
use alloc::string::String;
use core::str::FromStr;

use crate::ResolveError;
use crate::captures::Captures;
use crate::decode::decode;

/// The result of decoding a percent-escaped primitive capture.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrimitiveCapture<T> {
    /// The capture decoded and parsed successfully.
    Value(T),
    /// The capture's percent encoding or decoded UTF-8 was invalid.
    Undecodable,
    /// The decoded capture was not a value of the requested primitive type.
    Invalid,
}

/// Primitive path value decoded directly from raw and percent-escaped bytes.
///
/// Generated code selects the concrete implementation only for built-in
/// integer and Boolean captures. Custom `FromStr` types keep using
/// [`coerce_parse`] and [`parse`].
#[doc(hidden)]
pub trait PrimitivePath: Sized {
    /// Decodes and parses one capture without materializing an owned string.
    fn decode_path(raw: &str) -> PrimitiveCapture<Self>;
}

/// Coerces a static built-in primitive capture without allocating.
///
/// # Errors
/// [`ResolveError::UndecodableCapture`] on a malformed escape or invalid UTF-8;
/// [`ResolveError::InvalidCapture`] when primitive parsing fails.
#[inline]
pub fn coerce_primitive<'p, T: PrimitivePath>(raw: &'p str, field: &'static str) -> Result<T, ResolveError<'p>> {
    primitive_result(T::decode_path(raw), field)
}

// Static route coercion.

/// `String` field of a static route: percent-decoded, owned.
///
/// # Errors
/// [`ResolveError::UndecodableCapture`] on a malformed escape or invalid UTF-8.
#[inline]
pub fn coerce_owned<'p>(raw: &'p str, field: &'static str) -> Result<String, ResolveError<'p>> {
    decode(raw).map(Cow::into_owned).ok_or(ResolveError::UndecodableCapture(field))
}

/// `Cow<str>` field of a static route: borrowed when no decoding is needed.
///
/// # Errors
/// [`ResolveError::UndecodableCapture`] on a malformed escape or invalid UTF-8.
#[inline]
pub fn coerce_cow<'p>(raw: &'p str, field: &'static str) -> Result<Cow<'p, str>, ResolveError<'p>> {
    decode(raw).ok_or(ResolveError::UndecodableCapture(field))
}

/// `T: FromStr` field of a static route: percent-decoded, then parsed.
///
/// # Errors
/// [`ResolveError::UndecodableCapture`] on a malformed escape or invalid UTF-8;
/// [`ResolveError::InvalidCapture`] when parsing fails.
pub fn coerce_parse<'p, T: FromStr>(raw: &'p str, field: &'static str) -> Result<T, ResolveError<'p>> {
    let decoded = decode(raw).ok_or(ResolveError::UndecodableCapture(field))?;
    decoded.parse::<T>().map_err(|_err| ResolveError::InvalidCapture(field))
}

// Dynamic route coercion.

/// `String` field of a dynamic route: percent-decoded, owned.
///
/// # Errors
/// [`ResolveError::MissingCapture`] when absent; [`ResolveError::UndecodableCapture`] on a malformed
/// escape or invalid UTF-8.
#[inline]
pub fn owned(captures: &Captures<'_, '_, '_>, index: usize, field: &'static str) -> Result<String, ResolveError<'static>> {
    let raw = captures.get(index).ok_or(ResolveError::MissingCapture(field))?;
    decode(raw).map(Cow::into_owned).ok_or(ResolveError::UndecodableCapture(field))
}

/// `T: FromStr` field of a dynamic route: percent-decoded, then parsed.
///
/// # Errors
/// [`ResolveError::MissingCapture`] when absent; [`ResolveError::UndecodableCapture`] on a malformed
/// escape or invalid UTF-8; [`ResolveError::InvalidCapture`] when parsing fails.
pub fn parse<T: FromStr>(captures: &Captures<'_, '_, '_>, index: usize, field: &'static str) -> Result<T, ResolveError<'static>> {
    let raw = captures.get(index).ok_or(ResolveError::MissingCapture(field))?;
    let decoded = decode(raw).ok_or(ResolveError::UndecodableCapture(field))?;
    decoded.parse::<T>().map_err(|_err| ResolveError::InvalidCapture(field))
}

/// Coerces a dynamic built-in primitive capture without allocating.
///
/// # Errors
/// [`ResolveError::MissingCapture`] when absent; [`ResolveError::UndecodableCapture`] on a malformed
/// escape or invalid UTF-8; [`ResolveError::InvalidCapture`] when primitive parsing fails.
#[inline]
pub fn primitive<T: PrimitivePath>(captures: &Captures<'_, '_, '_>, index: usize, field: &'static str) -> Result<T, ResolveError<'static>> {
    let raw = captures.get(index).ok_or(ResolveError::MissingCapture(field))?;
    primitive_result(T::decode_path(raw), field)
}

fn primitive_result<T>(result: PrimitiveCapture<T>, field: &'static str) -> Result<T, ResolveError<'static>> {
    match result {
        PrimitiveCapture::Value(value) => Ok(value),
        PrimitiveCapture::Undecodable => Err(ResolveError::UndecodableCapture(field)),
        PrimitiveCapture::Invalid => Err(ResolveError::InvalidCapture(field)),
    }
}

// The macro keeps every generated primitive arm on one shared implementation
// while preserving the concrete return type known by codegen.
macro_rules! unsigned_primitive {
    ($( $ty:ty => $max:expr ),+ $(,)?) => {
        $(
            impl PrimitivePath for $ty {
                #[inline]
                fn decode_path(raw: &str) -> PrimitiveCapture<Self> {
                    if raw.find('%').is_none() {
                        return raw.parse().map_or(PrimitiveCapture::Invalid, PrimitiveCapture::Value);
                    }

                    decode_unsigned(raw, $max).map(|value| match <$ty>::try_from(value) {
                        Ok(value) => value,
                        Err(_) => unreachable!("decode_unsigned rejects magnitudes above the concrete primitive maximum"),
                    })
                }
            }
        )+
    };
}

macro_rules! signed_primitive {
    ($( $ty:ty => $max:expr ),+ $(,)?) => {
        $(
            impl PrimitivePath for $ty {
                #[inline]
                fn decode_path(raw: &str) -> PrimitiveCapture<Self> {
                    if raw.find('%').is_none() {
                        return raw.parse().map_or(PrimitiveCapture::Invalid, PrimitiveCapture::Value);
                    }

                    decode_signed(raw, $max).map(|value| {
                        if value.negative && value.magnitude == $max + 1 {
                            return <$ty>::MIN;
                        }
                        let Ok(magnitude) = <$ty>::try_from(value.magnitude) else {
                            unreachable!("decode_signed bounds magnitudes to the concrete primitive range");
                        };
                        if value.negative { -magnitude } else { magnitude }
                    })
                }
            }
        )+
    };
}

unsigned_primitive!(
    u8 => u128::from(u8::MAX),
    u16 => u128::from(u16::MAX),
    u32 => u128::from(u32::MAX),
    u64 => u128::from(u64::MAX),
    usize => usize::MAX as u128,
);

impl PrimitivePath for u128 {
    #[inline]
    fn decode_path(raw: &str) -> PrimitiveCapture<Self> {
        if raw.find('%').is_none() {
            return raw.parse().map_or(PrimitiveCapture::Invalid, PrimitiveCapture::Value);
        }

        decode_unsigned(raw, Self::MAX)
    }
}

signed_primitive!(
    i8 => i8::MAX as u128,
    i16 => i16::MAX as u128,
    i32 => i32::MAX as u128,
    i64 => i64::MAX as u128,
    i128 => i128::MAX as u128,
    isize => isize::MAX as u128,
);

impl PrimitivePath for bool {
    #[inline]
    fn decode_path(raw: &str) -> PrimitiveCapture<Self> {
        if raw.find('%').is_none() {
            return raw.parse().map_or(PrimitiveCapture::Invalid, PrimitiveCapture::Value);
        }

        let mut bytes = DecodedBytes::new(raw);
        let expected: &[u8] = match bytes.next() {
            Some(Ok(b't')) => b"rue",
            Some(Ok(b'f')) => b"alse",
            Some(Ok(_)) => return invalid_or_undecodable(bytes),
            Some(Err(())) => return PrimitiveCapture::Undecodable,
            None => return primitive_end(&bytes, None),
        };
        for &expected_byte in expected {
            match bytes.next() {
                Some(Ok(byte)) if byte == expected_byte => {}
                Some(Ok(_)) => return invalid_or_undecodable(bytes),
                Some(Err(())) => return PrimitiveCapture::Undecodable,
                None => return primitive_end(&bytes, None),
            }
        }
        match bytes.next() {
            Some(Ok(_)) => invalid_or_undecodable(bytes),
            Some(Err(())) => PrimitiveCapture::Undecodable,
            None => primitive_end(&bytes, Some(expected.len() == 3)),
        }
    }
}

impl<T> PrimitiveCapture<T> {
    fn map<U>(self, map: impl FnOnce(T) -> U) -> PrimitiveCapture<U> {
        match self {
            Self::Value(value) => PrimitiveCapture::Value(map(value)),
            Self::Undecodable => PrimitiveCapture::Undecodable,
            Self::Invalid => PrimitiveCapture::Invalid,
        }
    }
}

#[derive(Clone, Copy)]
struct SignedMagnitude {
    magnitude: u128,
    negative: bool,
}

fn decode_unsigned(raw: &str, max: u128) -> PrimitiveCapture<u128> {
    let mut bytes = DecodedBytes::new(raw);
    let Some(first) = bytes.next() else {
        return primitive_end(&bytes, None);
    };
    let Ok(first) = first else {
        return PrimitiveCapture::Undecodable;
    };
    let mut value = if first == b'+' {
        match bytes.next() {
            Some(Ok(byte)) => {
                let Some(digit) = digit(byte) else {
                    return invalid_or_undecodable(bytes);
                };
                u128::from(digit)
            }
            Some(Err(())) => return PrimitiveCapture::Undecodable,
            None => return primitive_end(&bytes, None),
        }
    } else {
        match digit(first) {
            Some(digit) => u128::from(digit),
            None => return invalid_or_undecodable(bytes),
        }
    };

    for byte in bytes.by_ref() {
        let Ok(byte) = byte else {
            return PrimitiveCapture::Undecodable;
        };
        let Some(digit) = digit(byte) else {
            return invalid_or_undecodable(bytes);
        };
        let Some(next) = value.checked_mul(10).and_then(|value| value.checked_add(u128::from(digit))) else {
            return invalid_or_undecodable(bytes);
        };
        if next > max {
            return invalid_or_undecodable(bytes);
        }
        value = next;
    }

    primitive_end(&bytes, Some(value))
}

fn decode_signed(raw: &str, positive_max: u128) -> PrimitiveCapture<SignedMagnitude> {
    let mut bytes = DecodedBytes::new(raw);
    let Some(first) = bytes.next() else {
        return primitive_end(&bytes, None);
    };
    let Ok(first) = first else {
        return PrimitiveCapture::Undecodable;
    };
    let (negative, mut value) = if matches!(first, b'+' | b'-') {
        let negative = first == b'-';
        match bytes.next() {
            Some(Ok(byte)) => {
                let Some(digit) = digit(byte) else {
                    return invalid_or_undecodable(bytes);
                };
                (negative, u128::from(digit))
            }
            Some(Err(())) => return PrimitiveCapture::Undecodable,
            None => return primitive_end(&bytes, None),
        }
    } else {
        match digit(first) {
            Some(digit) => (false, u128::from(digit)),
            None => return invalid_or_undecodable(bytes),
        }
    };
    let max = positive_max + u128::from(negative);

    for byte in bytes.by_ref() {
        let Ok(byte) = byte else {
            return PrimitiveCapture::Undecodable;
        };
        let Some(digit) = digit(byte) else {
            return invalid_or_undecodable(bytes);
        };
        let Some(next) = value.checked_mul(10).and_then(|value| value.checked_add(u128::from(digit))) else {
            return invalid_or_undecodable(bytes);
        };
        if next > max {
            return invalid_or_undecodable(bytes);
        }
        value = next;
    }

    primitive_end(
        &bytes,
        Some(SignedMagnitude {
            magnitude: value,
            negative,
        }),
    )
}

fn digit(byte: u8) -> Option<u8> {
    byte.is_ascii_digit().then(|| byte - b'0')
}

fn primitive_end<T>(bytes: &DecodedBytes<'_>, value: Option<T>) -> PrimitiveCapture<T> {
    if bytes.utf8_valid() {
        value.map_or(PrimitiveCapture::Invalid, PrimitiveCapture::Value)
    } else {
        PrimitiveCapture::Undecodable
    }
}

fn invalid_or_undecodable<T>(mut bytes: DecodedBytes<'_>) -> PrimitiveCapture<T> {
    for byte in bytes.by_ref() {
        if byte.is_err() {
            return PrimitiveCapture::Undecodable;
        }
    }
    primitive_end(&bytes, None)
}

struct DecodedBytes<'a> {
    bytes: &'a [u8],
    cursor: usize,
    utf8_remaining: u8,
    utf8_first_continuation: Option<(u8, u8)>,
    utf8_valid: bool,
}

impl<'a> DecodedBytes<'a> {
    fn new(raw: &'a str) -> Self {
        Self {
            bytes: raw.as_bytes(),
            cursor: 0,
            utf8_remaining: 0,
            utf8_first_continuation: None,
            utf8_valid: true,
        }
    }

    fn utf8_valid(&self) -> bool {
        self.utf8_valid && self.utf8_remaining == 0
    }

    fn record_utf8(&mut self, byte: u8) {
        if !self.utf8_valid {
            return;
        }
        if self.utf8_remaining != 0 {
            let (min, max) = self.utf8_first_continuation.take().unwrap_or((0x80, 0xBF));
            if !(min..=max).contains(&byte) {
                self.utf8_valid = false;
                return;
            }
            self.utf8_remaining -= 1;
            return;
        }

        match byte {
            0x00..=0x7F => {}
            0xC2..=0xDF => self.utf8_remaining = 1,
            0xE0 => {
                self.utf8_remaining = 2;
                self.utf8_first_continuation = Some((0xA0, 0xBF));
            }
            0xE1..=0xEC | 0xEE..=0xEF => self.utf8_remaining = 2,
            0xED => {
                self.utf8_remaining = 2;
                self.utf8_first_continuation = Some((0x80, 0x9F));
            }
            0xF0 => {
                self.utf8_remaining = 3;
                self.utf8_first_continuation = Some((0x90, 0xBF));
            }
            0xF1..=0xF3 => self.utf8_remaining = 3,
            0xF4 => {
                self.utf8_remaining = 3;
                self.utf8_first_continuation = Some((0x80, 0x8F));
            }
            _ => self.utf8_valid = false,
        }
    }
}

impl Iterator for DecodedBytes<'_> {
    type Item = Result<u8, ()>;

    // Gungraun shows this iterator step must inline to keep encoded primitive
    // decoding at least 5% below the allocating path on every error branch.
    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        let byte = *self.bytes.get(self.cursor)?;
        let decoded = if byte == b'%' {
            let Some(hi) = self.bytes.get(self.cursor + 1).copied().and_then(hex_value) else {
                self.cursor = self.bytes.len();
                return Some(Err(()));
            };
            let Some(lo) = self.bytes.get(self.cursor + 2).copied().and_then(hex_value) else {
                self.cursor = self.bytes.len();
                return Some(Err(()));
            };
            self.cursor += 3;
            hi * 16 + lo
        } else {
            self.cursor += 1;
            byte
        };
        self.record_utf8(decoded);
        Some(Ok(decoded))
    }
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}
