// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::cmp::Ordering;
use std::error::Error;
use std::fmt;
use std::hash::{Hash, Hasher};

use super::shared::validate_quality;
use crate::{DecodeMode, FieldName, validate};

/// An exact HTTP quality in integer thousandths, between zero and one.
///
/// Use [`QualityView`] for relaxed fractions that are not exact thousandths.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Quality(u16);

impl Quality {
    /// Zero quality: the member is unacceptable.
    pub const ZERO: Self = Self(0);

    /// Full quality, also the effective value when `q` is absent.
    pub const ONE: Self = Self(1000);

    /// Constructs an exact strict quality.
    ///
    /// # Errors
    ///
    /// Returns an error when `value` exceeds 1000.
    pub const fn from_thousandths(value: u16) -> Result<Self, InvalidQuality> {
        if value <= 1000 {
            Ok(Self(value))
        } else {
            Err(InvalidQuality { _private: () })
        }
    }

    /// Returns the exact integer number of thousandths.
    #[must_use]
    pub const fn thousandths(self) -> u16 {
        self.0
    }
}

impl fmt::Display for Quality {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        QualityView::from(*self).fmt(f)
    }
}

/// An invalid quality spelling or out-of-range number of thousandths.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct InvalidQuality {
    _private: (),
}

impl fmt::Display for InvalidQuality {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("quality must be an accepted decimal between zero and one")
    }
}

impl Error for InvalidQuality {}

/// An exact quality that cannot be represented in integer thousandths.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct InexactQuality {
    _private: (),
}

impl fmt::Display for InexactQuality {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("quality is not an exact number of thousandths")
    }
}

impl Error for InexactQuality {}

/// An exact borrowed quality, including arbitrarily long relaxed fractions.
///
/// Values exactly representable in thousandths use compact storage. Other
/// values borrow validated fractional digits. Equality, ordering and hashing
/// are numeric: `0.5`, `.50` and `0.5000` are equal. No rounding or floating
/// point is used. The original spelling remains in the header's raw members.
#[derive(Clone, Copy, Debug)]
pub struct QualityView<'a> {
    repr: Representation<'a>,
}

#[derive(Clone, Copy, Debug)]
enum Representation<'a> {
    Compact(Quality),
    Fraction(&'a [u8]),
}

impl<'a> QualityView<'a> {
    /// Zero quality.
    pub const ZERO: Self = Self {
        repr: Representation::Compact(Quality::ZERO),
    };

    /// Full quality.
    pub const ONE: Self = Self {
        repr: Representation::Compact(Quality::ONE),
    };

    /// Parses one quality using the header's strict or relaxed grammar.
    ///
    /// Relaxed mode accepts leading-dot fractions and additional fractional
    /// digits, with no additional precision limit.
    ///
    /// # Errors
    ///
    /// Returns an error for a malformed or out-of-range quality.
    pub fn parse(bytes: &'a [u8], mode: DecodeMode) -> Result<Self, InvalidQuality> {
        validate_quality(bytes, &FieldName::Accept, mode == DecodeMode::Relaxed).map_err(|_invalid| InvalidQuality { _private: () })?;
        Ok(Self::from_validated(validate::trim_ows(bytes)))
    }

    pub(super) fn from_validated(bytes: &'a [u8]) -> Self {
        if bytes[0] == b'1' {
            return Self::ONE;
        }
        let ([b'0', b'.', fraction @ ..] | [b'.', fraction @ ..]) = bytes else {
            return Self::ZERO;
        };
        let significant = fraction.iter().rposition(|digit| *digit != b'0').map_or(0, |last| last + 1);
        let fraction = &fraction[..significant];
        if significant > 3 {
            return Self {
                repr: Representation::Fraction(fraction),
            };
        }
        let mut thousandths = 0;
        for index in 0..3 {
            thousandths = thousandths * 10 + u16::from(fraction.get(index).copied().unwrap_or(b'0') - b'0');
        }
        Self::from(Quality(thousandths))
    }

    /// Converts without rounding.
    ///
    /// # Errors
    ///
    /// Returns [`InexactQuality`] if any digit beyond thousandths is nonzero.
    pub const fn to_quality(self) -> Result<Quality, InexactQuality> {
        match self.repr {
            Representation::Compact(value) => Ok(value),
            Representation::Fraction(_) => Err(InexactQuality { _private: () }),
        }
    }

    /// Whether the exact quality is zero.
    #[must_use]
    pub const fn is_zero(self) -> bool {
        matches!(self.repr, Representation::Compact(Quality::ZERO))
    }

    /// Whether the exact quality is one.
    #[must_use]
    pub const fn is_one(self) -> bool {
        matches!(self.repr, Representation::Compact(Quality::ONE))
    }

    fn fraction_len(self) -> usize {
        match self.repr {
            Representation::Fraction(digits) => digits.len(),
            Representation::Compact(Quality(0 | 1000)) => 0,
            Representation::Compact(Quality(value)) if value.is_multiple_of(100) => 1,
            Representation::Compact(Quality(value)) if value.is_multiple_of(10) => 2,
            Representation::Compact(_) => 3,
        }
    }

    fn digit(self, index: usize) -> u8 {
        match self.repr {
            Representation::Fraction(digits) => digits.get(index).copied().unwrap_or(b'0'),
            Representation::Compact(Quality(value)) => {
                let digit = match index {
                    0 => value / 100 % 10,
                    1 => value / 10 % 10,
                    2 => value % 10,
                    _ => 0,
                };
                u8::try_from(digit).expect("a decimal digit is at most nine") + b'0'
            }
        }
    }

    pub(super) fn encoded_len(self) -> usize {
        let fraction = self.fraction_len();
        if fraction == 0 { 1 } else { fraction + 2 }
    }

    pub(super) fn append_to(self, bytes: &mut Vec<u8>) {
        bytes.push(if self.is_one() { b'1' } else { b'0' });
        let fraction = self.fraction_len();
        if fraction != 0 {
            bytes.push(b'.');
            for index in 0..fraction {
                bytes.push(self.digit(index));
            }
        }
    }
}

impl From<Quality> for QualityView<'_> {
    fn from(value: Quality) -> Self {
        Self {
            repr: Representation::Compact(value),
        }
    }
}

impl TryFrom<QualityView<'_>> for Quality {
    type Error = InexactQuality;

    fn try_from(value: QualityView<'_>) -> Result<Self, Self::Error> {
        value.to_quality()
    }
}

impl PartialEq for QualityView<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for QualityView<'_> {}

impl PartialOrd for QualityView<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for QualityView<'_> {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self.repr, other.repr) {
            (Representation::Compact(left), Representation::Compact(right)) => left.cmp(&right),
            // Canonical fractions have no trailing zeroes, so a shorter
            // prefix sorts before the longer exact decimal value.
            (Representation::Fraction(left), Representation::Fraction(right)) => left.cmp(right),
            (Representation::Compact(left), Representation::Fraction(right)) => compare_compact_fraction(left, right),
            (Representation::Fraction(left), Representation::Compact(right)) => compare_compact_fraction(right, left).reverse(),
        }
    }
}

fn compare_compact_fraction(compact: Quality, fraction: &[u8]) -> Ordering {
    let thousandths = u16::from(fraction[0] - b'0') * 100 + u16::from(fraction[1] - b'0') * 10 + u16::from(fraction[2] - b'0');
    // Fraction storage has a nonzero digit beyond thousandths; equality of
    // the first three digits therefore puts the compact value strictly first.
    compact.thousandths().cmp(&thousandths).then(Ordering::Less)
}

impl Hash for QualityView<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.is_one().hash(state);
        self.fraction_len().hash(state);
        for index in 0..self.fraction_len() {
            self.digit(index).hash(state);
        }
    }
}

impl fmt::Display for QualityView<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(if self.is_one() { "1" } else { "0" })?;
        let fraction = self.fraction_len();
        if fraction != 0 {
            f.write_str(".")?;
            for index in 0..fraction {
                write!(f, "{}", char::from(self.digit(index)))?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::{Quality, QualityView};
    use crate::DecodeMode;

    #[test]
    fn fractional_digits_are_zero_padded_beyond_the_retained_precision() {
        let compact = QualityView::from(Quality::from_thousandths(125).unwrap());
        let fraction = QualityView::parse(b"0.1251", DecodeMode::Relaxed).unwrap();
        assert_eq!([0, 1, 2, 3, 4].map(|index| compact.digit(index)), *b"12500");
        assert_eq!([0, 1, 2, 3, 4].map(|index| fraction.digit(index)), *b"12510");
        assert_eq!(compact.digit(usize::MAX), b'0');
        assert_eq!(fraction.digit(usize::MAX), b'0');
    }
}
