// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use core::fmt::Debug;
use std::fmt::Write;

use rustc_hash::FxHashMap;

use crate::simple_redactor::{SimpleRedactor, SimpleRedactorMode};
use crate::{DataClass, IntoDataClass};

pub mod simple_redactor;

#[cfg(feature = "xxh3")]
pub mod xxh3_redactor;

#[cfg(feature = "rapidhash")]
pub mod rapidhash_redactor;

/// Represents types that can redact data.
pub trait Redactor {
    /// Redacts the given value and writes the results to the given output sink.
    ///
    /// # Errors
    ///
    /// This function should return [`Err`] if, and only if, the provided [`Formatter`](std::fmt::Formatter) returns [`Err`]. String redaction is considered an infallible operation;
    /// this function only returns a [`std::fmt::Result`] because writing to the underlying stream might fail and it must provide a way to propagate the fact that an error
    /// has occurred back up the stack.
    fn redact(&self, data_class: &DataClass, value: &str, output: &mut dyn Write) -> std::fmt::Result;
}

pub(crate) struct Redactors {
    redactors: FxHashMap<DataClass, Box<dyn Redactor + Send + Sync>>,
    fallback: Box<dyn Redactor + Send + Sync>,
}

/// Type holding all redactors registered for different data classes.
impl Redactors {
    #[must_use]
    pub fn get(&self, data_class: &DataClass) -> Option<&(dyn Redactor + Send + Sync)> {
        self.redactors.get(data_class).map(AsRef::as_ref)
    }

    #[must_use]
    fn fallback(&self) -> &(dyn Redactor + Send + Sync) {
        self.fallback.as_ref()
    }

    #[must_use]
    pub fn get_or_fallback(&self, data_class: &DataClass) -> &(dyn Redactor + Send + Sync) {
        self.get(data_class).unwrap_or_else(|| self.fallback())
    }

    #[cfg(test)]
    #[must_use]
    pub fn len(&self) -> usize {
        self.redactors.len()
    }

    pub fn shrink(&mut self) {
        self.redactors.shrink_to_fit();
    }

    pub fn insert(&mut self, data_class: impl IntoDataClass, redactor: impl Redactor + Send + Sync + 'static) {
        self.redactors.insert(data_class.into_data_class(), Box::new(redactor));
    }

    pub fn set_fallback(&mut self, redactor: impl Redactor + Send + Sync + 'static) {
        self.fallback = Box::new(redactor);
    }
}

impl Default for Redactors {
    fn default() -> Self {
        Self {
            redactors: FxHashMap::default(),
            fallback: Box::new(SimpleRedactor::with_mode(SimpleRedactorMode::Insert("*".into()))),
        }
    }
}

impl Debug for Redactors {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_list().entries(self.redactors.keys()).finish()
    }
}

#[cfg(any(feature = "xxh3", feature = "rapidhash"))]
#[inline]
pub fn u64_to_hex_array<const N: usize>(mut value: u64) -> [u8; N] {
    const HEX_LOWER_CHARS: &[u8; 16] = b"0123456789abcdef";

    let mut buffer = [0u8; N];
    for e in buffer.iter_mut().rev() {
        *e = HEX_LOWER_CHARS[(value & 0x0f) as usize];
        value >>= 4;
    }

    buffer
}

#[cfg(test)]
mod tests {
    use data_privacy_macros::taxonomy;
    use rustc_hash::FxBuildHasher;

    use super::*;

    #[cfg(any(feature = "xxh3", feature = "rapidhash"))]
    #[test]
    fn test_u64_to_hex_array() {
        let result = u64_to_hex_array(0x1234_5678_9abc_def0);
        let expected = b"123456789abcdef0";
        assert_eq!(result, *expected);

        let result = u64_to_hex_array(0);
        let expected = b"0000000000000000";
        assert_eq!(result, *expected);

        let result = u64_to_hex_array(u64::MAX);
        let expected = b"ffffffffffffffff";
        assert_eq!(result, *expected);
    }

    struct TestRedactor;

    impl Redactor for TestRedactor {
        fn redact(&self, _data_class: &DataClass, value: &str, output: &mut dyn Write) -> std::fmt::Result {
            write!(output, "{value}tomato")
        }
    }

    #[taxonomy(test)]
    enum TestTaxonomy {
        Sensitive,
        Insensitive,
    }

    #[test]
    fn test_redactor_shrink() {
        let mut redactors = Redactors {
            redactors: FxHashMap::with_capacity_and_hasher(42, FxBuildHasher),
            ..Default::default()
        };

        redactors.insert(
            TestTaxonomy::Sensitive.data_class(),
            SimpleRedactor::with_mode(SimpleRedactorMode::PassthroughAndTag),
        );
        redactors.insert(
            TestTaxonomy::Insensitive.data_class(),
            SimpleRedactor::with_mode(SimpleRedactorMode::Replace('#')),
        );

        // Check initial size
        assert_eq!(redactors.len(), 2);
        assert!(redactors.redactors.capacity() >= 42, "Initial capacity should be at least 42");

        // Shrink the redactors
        redactors.shrink();

        // Check size after shrinking
        assert_eq!(redactors.len(), 2, "Shrink should not change the number of redactors");
        assert!(redactors.redactors.capacity() < 42, "Capacity should shrink below 42");
    }

    #[test]
    fn test_exact_len_default_behavior() {
        let redactor = TestRedactor;
        let mut output_buffer = String::new();
        _ = redactor.redact(&TestTaxonomy::Sensitive.data_class(), "test_value", &mut output_buffer);

        assert_eq!(output_buffer, "test_valuetomato");
    }

    #[test]
    fn test_fallback_isnt_redactor() {
        let fallback_redactor = SimpleRedactor::with_mode(SimpleRedactorMode::Erase);
        let mut redactors = Redactors::default();
        redactors.set_fallback(fallback_redactor);
        assert_eq!(redactors.len(), 0);
    }
}
