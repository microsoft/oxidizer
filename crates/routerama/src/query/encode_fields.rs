// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use core::fmt;

use super::encoder::is_safe;
use super::{Encoder, Error};

/// Runtime contract implemented by `ToQuery` derive output.
#[doc(hidden)]
pub trait EncodeFields {
    /// Writes the fields of this schema to an encoder.
    fn encode_fields<W: fmt::Write>(&self, encoder: &mut Encoder<'_, W>) -> Result<(), Error>;

    /// Returns the exact generated encoded length when it is known without
    /// formatting user-defined values.
    fn encoded_length_hint(&self) -> Option<usize> {
        None
    }
}

/// Saturating encoded-length estimate assembled by generated query encoders.
#[doc(hidden)]
#[derive(Debug, Default)]
pub struct EncodedLengthHint {
    length: usize,
}

impl EncodedLengthHint {
    /// Creates an empty estimate.
    #[must_use]
    #[inline]
    pub const fn new() -> Self {
        Self { length: 0 }
    }

    /// Adds one encoded pair.
    #[inline]
    pub fn pair(&mut self, encoded_parameter: &'static str, value_length: usize) {
        if self.length != 0 {
            self.length = self.length.saturating_add(1);
        }
        self.length = self
            .length
            .saturating_add(encoded_parameter.len())
            .saturating_add(1)
            .saturating_add(value_length);
    }

    /// Adds a flattened schema estimate.
    #[inline]
    pub fn flatten(&mut self, nested_length: usize) {
        if nested_length == 0 {
            return;
        }
        if self.length != 0 {
            self.length = self.length.saturating_add(1);
        }
        self.length = self.length.saturating_add(nested_length);
    }

    /// Finishes the estimate.
    #[must_use]
    #[inline]
    pub const fn finish(self) -> usize {
        self.length
    }
}

/// Returns the exact encoded length of a string value.
#[doc(hidden)]
#[must_use]
#[inline]
pub fn encoded_str_length(value: &str) -> usize {
    let escaped = value
        .as_bytes()
        .iter()
        .map(|&byte| usize::from(ENCODED_EXPANSION[usize::from(byte)]))
        .sum::<usize>();
    value.len().saturating_add(escaped.saturating_mul(2))
}

// The obvious predicate branch per byte regressed the long escaped-string
// control by 11.8% instructions. This small lookup keeps the exact pass
// branch-free; a raw-length estimate still required output reallocations.
const ENCODED_EXPANSION: [u8; 256] = {
    let mut expansion = [0; 256];
    let mut byte = 0;
    while byte < expansion.len() {
        let value = byte.to_le_bytes()[0];
        expansion[byte] = if !is_safe(value) && value != b' ' { 1 } else { 0 };
        byte += 1;
    }
    expansion
};

macro_rules! unsigned_lengths {
    ($($function:ident: $ty:ty),+ $(,)?) => {
        $(
            #[doc(hidden)]
            #[must_use]
            #[inline]
            pub fn $function(value: $ty) -> usize {
                if value == 0 {
                    1
                } else {
                    value.ilog10() as usize + 1
                }
            }
        )+
    };
}

macro_rules! signed_lengths {
    ($($function:ident: $ty:ty),+ $(,)?) => {
        $(
            #[doc(hidden)]
            #[must_use]
            #[inline]
            pub fn $function(value: $ty) -> usize {
                let magnitude = value.unsigned_abs();
                let digits = if magnitude == 0 {
                    1
                } else {
                    magnitude.ilog10() as usize + 1
                };
                digits + usize::from(value.is_negative())
            }
        )+
    };
}

unsigned_lengths!(
    encoded_u8_length: u8,
    encoded_u16_length: u16,
    encoded_u32_length: u32,
    encoded_u64_length: u64,
    encoded_u128_length: u128,
    encoded_usize_length: usize,
);
signed_lengths!(
    encoded_i8_length: i8,
    encoded_i16_length: i16,
    encoded_i32_length: i32,
    encoded_i64_length: i64,
    encoded_i128_length: i128,
    encoded_isize_length: isize,
);

/// Returns the exact encoded length of a Boolean value.
#[doc(hidden)]
#[must_use]
#[inline]
pub const fn encoded_bool_length(value: bool) -> usize {
    if value { 4 } else { 5 }
}

#[cfg(test)]
mod tests {
    use alloc::string::{String, ToString as _};

    use super::*;
    use crate::query::QueryLimits;

    #[test]
    fn string_lengths_match_the_encoder() {
        for value in ["", "safe-AZ_09.*-", "a b/c~", "café"] {
            let mut output = String::new();
            let mut encoder = Encoder::new(&mut output, QueryLimits::UNLIMITED);
            encoder.pair_str("", "", value).expect("test value encodes");
            assert_eq!(encoded_str_length(value), output.len() - 1);
        }
    }

    #[test]
    fn primitive_lengths_cover_zero_and_boundaries() {
        macro_rules! assert_lengths {
            ($function:ident: $($value:expr),+ $(,)?) => {
                $(assert_eq!($function($value), $value.to_string().len());)+
            };
        }

        assert_lengths!(encoded_u8_length: 0_u8, u8::MAX);
        assert_lengths!(encoded_u16_length: 0_u16, u16::MAX);
        assert_lengths!(encoded_u32_length: 0_u32, u32::MAX);
        assert_lengths!(encoded_u64_length: 0_u64, u64::MAX);
        assert_lengths!(encoded_u128_length: 0_u128, u128::MAX);
        assert_lengths!(encoded_usize_length: 0_usize, usize::MAX);
        assert_lengths!(encoded_i8_length: 0_i8, i8::MIN, i8::MAX);
        assert_lengths!(encoded_i16_length: 0_i16, i16::MIN, i16::MAX);
        assert_lengths!(encoded_i32_length: 0_i32, i32::MIN, i32::MAX);
        assert_lengths!(encoded_i64_length: 0_i64, i64::MIN, i64::MAX);
        assert_lengths!(encoded_i128_length: 0_i128, i128::MIN, i128::MAX);
        assert_lengths!(encoded_isize_length: 0_isize, isize::MIN, isize::MAX);
        assert_eq!(encoded_bool_length(false), 5);
        assert_eq!(encoded_bool_length(true), 4);
    }
}
