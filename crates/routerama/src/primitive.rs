// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

// Query decoding needs `+` translation, exact escape offsets, and decoded-byte
// accounting. Keeping that state in the existing path iterator regressed
// encoded path captures in Callgrind, so this query adapter preserves the same
// checked primitive parsing rules without changing the path hot loop.

pub(crate) enum DecodeError {
    InvalidEncoding(usize),
    InvalidUtf8,
}

pub(crate) struct DecodedPrimitive<T> {
    decoded_len: usize,
    result: Result<Option<T>, DecodeError>,
}

impl<T> DecodedPrimitive<T> {
    pub(crate) fn into_parts(self) -> (usize, Result<Option<T>, DecodeError>) {
        (self.decoded_len, self.result)
    }

    fn map<U>(self, map: impl FnOnce(T) -> U) -> DecodedPrimitive<U> {
        DecodedPrimitive {
            decoded_len: self.decoded_len,
            result: self.result.map(|value| value.map(map)),
        }
    }
}

pub(crate) trait Primitive: Sized {
    fn decode_encoded<const PLUS_AS_SPACE: bool, const TRACK_LENGTH: bool>(raw: &str) -> DecodedPrimitive<Self>;
}

pub(crate) fn decode_encoded<T: Primitive, const PLUS_AS_SPACE: bool, const TRACK_LENGTH: bool>(raw: &str) -> DecodedPrimitive<T> {
    T::decode_encoded::<PLUS_AS_SPACE, TRACK_LENGTH>(raw)
}

macro_rules! unsigned_primitive {
    ($( $ty:ty => $max:expr ),+ $(,)?) => {
        $(
            impl Primitive for $ty {
                fn decode_encoded<const PLUS_AS_SPACE: bool, const TRACK_LENGTH: bool>(raw: &str) -> DecodedPrimitive<Self> {
                    decode_unsigned::<PLUS_AS_SPACE, TRACK_LENGTH>(raw, $max).map(|value| match <$ty>::try_from(value) {
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
            impl Primitive for $ty {
                fn decode_encoded<const PLUS_AS_SPACE: bool, const TRACK_LENGTH: bool>(raw: &str) -> DecodedPrimitive<Self> {
                    decode_signed::<PLUS_AS_SPACE, TRACK_LENGTH>(raw, $max).map(|value| {
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

impl Primitive for u128 {
    fn decode_encoded<const PLUS_AS_SPACE: bool, const TRACK_LENGTH: bool>(raw: &str) -> DecodedPrimitive<Self> {
        decode_unsigned::<PLUS_AS_SPACE, TRACK_LENGTH>(raw, Self::MAX)
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

impl Primitive for bool {
    fn decode_encoded<const PLUS_AS_SPACE: bool, const TRACK_LENGTH: bool>(raw: &str) -> DecodedPrimitive<Self> {
        let mut bytes = DecodedBytes::<PLUS_AS_SPACE, TRACK_LENGTH>::new(raw);
        let expected: &[u8] = match bytes.next() {
            Some(Ok(b't')) => b"rue",
            Some(Ok(b'f')) => b"alse",
            Some(Ok(_)) => return invalid(bytes),
            Some(Err(error)) => return undecodable(&bytes, error),
            None => return finish(&bytes, None),
        };
        for &expected_byte in expected {
            match bytes.next() {
                Some(Ok(byte)) if byte == expected_byte => {}
                Some(Ok(_)) => return invalid(bytes),
                Some(Err(error)) => return undecodable(&bytes, error),
                None => return finish(&bytes, None),
            }
        }
        match bytes.next() {
            Some(Ok(_)) => invalid(bytes),
            Some(Err(error)) => undecodable(&bytes, error),
            None => finish(&bytes, Some(expected.len() == 3)),
        }
    }
}

#[derive(Clone, Copy)]
struct SignedMagnitude {
    magnitude: u128,
    negative: bool,
}

fn decode_unsigned<const PLUS_AS_SPACE: bool, const TRACK_LENGTH: bool>(raw: &str, max: u128) -> DecodedPrimitive<u128> {
    let mut bytes = DecodedBytes::<PLUS_AS_SPACE, TRACK_LENGTH>::new(raw);
    let first = match bytes.next() {
        Some(Ok(first)) => first,
        Some(Err(error)) => return undecodable(&bytes, error),
        None => return finish(&bytes, None),
    };
    let mut value = if first == b'+' {
        match bytes.next() {
            Some(Ok(byte)) => {
                let Some(digit) = digit(byte) else {
                    return invalid(bytes);
                };
                u128::from(digit)
            }
            Some(Err(error)) => return undecodable(&bytes, error),
            None => return finish(&bytes, None),
        }
    } else {
        match digit(first) {
            Some(digit) => u128::from(digit),
            None => return invalid(bytes),
        }
    };

    while let Some(byte) = bytes.next() {
        let byte = match byte {
            Ok(byte) => byte,
            Err(error) => return undecodable(&bytes, error),
        };
        let Some(digit) = digit(byte) else {
            return invalid(bytes);
        };
        let Some(next) = value.checked_mul(10).and_then(|value| value.checked_add(u128::from(digit))) else {
            return invalid(bytes);
        };
        if next > max {
            return invalid(bytes);
        }
        value = next;
    }

    finish(&bytes, Some(value))
}

fn decode_signed<const PLUS_AS_SPACE: bool, const TRACK_LENGTH: bool>(raw: &str, positive_max: u128) -> DecodedPrimitive<SignedMagnitude> {
    let mut bytes = DecodedBytes::<PLUS_AS_SPACE, TRACK_LENGTH>::new(raw);
    let first = match bytes.next() {
        Some(Ok(first)) => first,
        Some(Err(error)) => return undecodable(&bytes, error),
        None => return finish(&bytes, None),
    };
    let (negative, mut value) = if matches!(first, b'+' | b'-') {
        let negative = first == b'-';
        match bytes.next() {
            Some(Ok(byte)) => {
                let Some(digit) = digit(byte) else {
                    return invalid(bytes);
                };
                (negative, u128::from(digit))
            }
            Some(Err(error)) => return undecodable(&bytes, error),
            None => return finish(&bytes, None),
        }
    } else {
        match digit(first) {
            Some(digit) => (false, u128::from(digit)),
            None => return invalid(bytes),
        }
    };
    let max = positive_max + u128::from(negative);

    while let Some(byte) = bytes.next() {
        let byte = match byte {
            Ok(byte) => byte,
            Err(error) => return undecodable(&bytes, error),
        };
        let Some(digit) = digit(byte) else {
            return invalid(bytes);
        };
        let Some(next) = value.checked_mul(10).and_then(|value| value.checked_add(u128::from(digit))) else {
            return invalid(bytes);
        };
        if next > max {
            return invalid(bytes);
        }
        value = next;
    }

    finish(
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

fn finish<T, const PLUS_AS_SPACE: bool, const TRACK_LENGTH: bool>(
    bytes: &DecodedBytes<'_, PLUS_AS_SPACE, TRACK_LENGTH>,
    value: Option<T>,
) -> DecodedPrimitive<T> {
    DecodedPrimitive {
        decoded_len: bytes.decoded_len,
        result: if bytes.utf8_valid() {
            Ok(value)
        } else {
            Err(DecodeError::InvalidUtf8)
        },
    }
}

fn invalid<T, const PLUS_AS_SPACE: bool, const TRACK_LENGTH: bool>(
    mut bytes: DecodedBytes<'_, PLUS_AS_SPACE, TRACK_LENGTH>,
) -> DecodedPrimitive<T> {
    for byte in bytes.by_ref() {
        if let Err(error) = byte {
            return undecodable(&bytes, error);
        }
    }
    finish(&bytes, None)
}

fn undecodable<T, const PLUS_AS_SPACE: bool, const TRACK_LENGTH: bool>(
    bytes: &DecodedBytes<'_, PLUS_AS_SPACE, TRACK_LENGTH>,
    error: DecodeError,
) -> DecodedPrimitive<T> {
    DecodedPrimitive {
        decoded_len: bytes.decoded_len,
        result: Err(error),
    }
}

struct DecodedBytes<'a, const PLUS_AS_SPACE: bool, const TRACK_LENGTH: bool> {
    bytes: &'a [u8],
    cursor: usize,
    decoded_len: usize,
    utf8_remaining: u8,
    utf8_first_continuation: Option<(u8, u8)>,
    utf8_valid: bool,
}

impl<'a, const PLUS_AS_SPACE: bool, const TRACK_LENGTH: bool> DecodedBytes<'a, PLUS_AS_SPACE, TRACK_LENGTH> {
    fn new(raw: &'a str) -> Self {
        Self {
            bytes: raw.as_bytes(),
            cursor: 0,
            decoded_len: 0,
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

impl<const PLUS_AS_SPACE: bool, const TRACK_LENGTH: bool> Iterator for DecodedBytes<'_, PLUS_AS_SPACE, TRACK_LENGTH> {
    type Item = Result<u8, DecodeError>;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        let byte = *self.bytes.get(self.cursor)?;
        let decoded = if byte == b'%' {
            let escape_offset = self.cursor;
            let Some(hi) = self.bytes.get(self.cursor + 1).copied().and_then(hex_value) else {
                self.cursor = self.bytes.len();
                return Some(Err(DecodeError::InvalidEncoding(escape_offset)));
            };
            let Some(lo) = self.bytes.get(self.cursor + 2).copied().and_then(hex_value) else {
                self.cursor = self.bytes.len();
                return Some(Err(DecodeError::InvalidEncoding(escape_offset)));
            };
            self.cursor += 3;
            hi * 16 + lo
        } else {
            self.cursor += 1;
            if PLUS_AS_SPACE && byte == b'+' { b' ' } else { byte }
        };
        if TRACK_LENGTH {
            self.decoded_len += 1;
        }
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
