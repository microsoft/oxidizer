// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Owned encoded field values.

use std::hash::{Hash, Hasher};
use std::{fmt, option, slice, vec};

use crate::FieldValue;

/// An iterator over borrowed encoded field values.
///
/// # Examples
///
/// ```rust
/// use http_headers::FieldValue;
/// use http_headers::sink::{EncodedValues, EncodedValuesIter};
///
/// let values = EncodedValues::from_vec(vec![
///     FieldValue::from_static("gzip"),
///     FieldValue::from_static("br"),
/// ]);
/// let mut iter: EncodedValuesIter<'_> = values.iter();
/// assert_eq!(iter.len(), 2);
/// assert_eq!(iter.next().expect("first value"), "gzip");
/// assert_eq!(iter.next().expect("second value"), "br");
/// assert!(iter.next().is_none());
/// ```
pub struct EncodedValuesIter<'a> {
    first: option::Iter<'a, FieldValue>,
    rest: slice::Iter<'a, FieldValue>,
}

/// An iterator over mutably borrowed encoded field values.
///
/// # Examples
///
/// ```rust
/// use http_headers::sink::{EncodedValues, EncodedValuesIterMut};
/// use http_headers::{FieldSensitivity, FieldValue};
///
/// let mut values = EncodedValues::from_vec(vec![
///     FieldValue::from_static("gzip"),
///     FieldValue::from_static("br"),
/// ]);
/// let mut iter: EncodedValuesIterMut<'_> = values.iter_mut();
/// assert_eq!(iter.len(), 2);
/// iter.next()
///     .expect("first mutable value")
///     .set_sensitivity(FieldSensitivity::Sensitive);
/// drop(iter);
/// assert!(values.iter().next().expect("first value").is_sensitive());
/// ```
pub struct EncodedValuesIterMut<'a> {
    first: option::IterMut<'a, FieldValue>,
    rest: slice::IterMut<'a, FieldValue>,
}

/// An owning iterator over encoded field values.
///
/// # Examples
///
/// ```rust
/// use http_headers::FieldValue;
/// use http_headers::sink::{EncodedValues, EncodedValuesIntoIter};
///
/// let values = EncodedValues::from_vec(vec![
///     FieldValue::from_static("gzip"),
///     FieldValue::from_static("br"),
/// ]);
/// let mut iter: EncodedValuesIntoIter = values.into_iter();
/// assert_eq!(iter.len(), 2);
/// assert_eq!(iter.next().expect("first value"), "gzip");
/// assert_eq!(iter.next().expect("second value"), "br");
/// assert!(iter.next().is_none());
/// ```
pub struct EncodedValuesIntoIter {
    first: option::IntoIter<FieldValue>,
    rest: vec::IntoIter<FieldValue>,
}

/// Owned field values ready to be stored by a [`crate::sink::FieldSink`].
///
/// The collection holds values only; the field name is supplied separately by
/// [`FieldSink::set_values`]. Each value becomes one field line once stored.
///
/// [`FieldSink::set_values`]: crate::sink::FieldSink::set_values
///
/// [`Debug`](std::fmt::Debug) reports only the number of values and never
/// exposes their contents.
///
/// # Examples
///
/// ```rust
/// let values =
///     http_headers::sink::EncodedValues::single(http_headers::FieldValue::from_static("gzip"));
/// assert_eq!(values.len(), 1);
/// ```
#[derive(Clone, Default)]
pub struct EncodedValues {
    first: Option<FieldValue>,
    rest: Vec<FieldValue>,
}

impl fmt::Debug for EncodedValues {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EncodedValues").field("value_count", &self.len()).finish()
    }
}

impl PartialEq for EncodedValues {
    fn eq(&self, other: &Self) -> bool {
        self.iter().eq(other.iter())
    }
}

impl Eq for EncodedValues {}

impl PartialOrd for EncodedValues {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for EncodedValues {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.iter().cmp(other.iter())
    }
}

impl Hash for EncodedValues {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.len().hash(state);
        self.iter().for_each(|value| value.hash(state));
    }
}

impl EncodedValues {
    /// Creates an empty collection.
    ///
    /// # Examples
    ///
    /// ```rust
    /// assert!(http_headers::sink::EncodedValues::new().is_empty());
    /// ```
    #[must_use]
    pub const fn new() -> Self {
        Self {
            first: None,
            rest: Vec::new(),
        }
    }

    /// Creates a collection containing one field value.
    ///
    /// # Examples
    ///
    /// ```rust
    /// let values =
    ///     http_headers::sink::EncodedValues::single(http_headers::FieldValue::from_static("gzip"));
    /// assert_eq!(values.len(), 1);
    /// ```
    #[must_use]
    pub fn single(value: FieldValue) -> Self {
        Self {
            first: Some(value),
            rest: Vec::new(),
        }
    }

    /// Creates a collection from field values in their existing order.
    ///
    /// # Examples
    ///
    /// ```rust
    /// let values =
    ///     http_headers::sink::EncodedValues::from_vec(vec![http_headers::FieldValue::from_static(
    ///         "gzip",
    ///     )]);
    /// assert_eq!(values.len(), 1);
    /// ```
    #[must_use]
    pub fn from_vec(values: Vec<FieldValue>) -> Self {
        Self { first: None, rest: values }
    }

    /// Appends a field value.
    ///
    /// # Examples
    ///
    /// ```rust
    /// let mut values = http_headers::sink::EncodedValues::new();
    /// values.push(http_headers::FieldValue::from_static("gzip"));
    /// assert_eq!(values.len(), 1);
    /// ```
    pub fn push(&mut self, value: FieldValue) {
        if self.first.is_some() || !self.rest.is_empty() {
            self.rest.push(value);
        } else {
            self.first = Some(value);
        }
    }

    /// Returns the number of field values.
    ///
    /// # Examples
    ///
    /// ```rust
    /// let values =
    ///     http_headers::sink::EncodedValues::single(http_headers::FieldValue::from_static("gzip"));
    /// assert_eq!(values.len(), 1);
    /// ```
    #[must_use]
    pub fn len(&self) -> usize {
        usize::from(self.first.is_some()) + self.rest.len()
    }

    /// Returns whether no field values were encoded.
    ///
    /// # Examples
    ///
    /// ```rust
    /// assert!(http_headers::sink::EncodedValues::new().is_empty());
    /// ```
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.first.is_none() && self.rest.is_empty()
    }

    /// Iterates the encoded field values.
    ///
    /// # Examples
    ///
    /// ```rust
    /// let values =
    ///     http_headers::sink::EncodedValues::single(http_headers::FieldValue::from_static("gzip"));
    /// assert!(values.iter().any(|value| value == "gzip"));
    /// ```
    pub fn iter(&self) -> EncodedValuesIter<'_> {
        EncodedValuesIter {
            first: self.first.iter(),
            rest: self.rest.iter(),
        }
    }

    /// Mutably iterates the encoded field values.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::FieldSensitivity;
    ///
    /// let mut values =
    ///     http_headers::sink::EncodedValues::single(http_headers::FieldValue::from_static("gzip"));
    /// values
    ///     .iter_mut()
    ///     .for_each(|value| value.set_sensitivity(FieldSensitivity::Sensitive));
    /// assert!(values.iter().all(|value| value.is_sensitive()));
    /// ```
    pub fn iter_mut(&mut self) -> EncodedValuesIterMut<'_> {
        EncodedValuesIterMut {
            first: self.first.iter_mut(),
            rest: self.rest.iter_mut(),
        }
    }
}

macro_rules! impl_encoded_iterator {
    ($type:ident $(<$lifetime:lifetime>)?, $item:ty) => {
        impl$(<$lifetime>)? Iterator for $type$(<$lifetime>)? {
            type Item = $item;

            fn next(&mut self) -> Option<Self::Item> {
                self.first.next().or_else(|| self.rest.next())
            }

            fn size_hint(&self) -> (usize, Option<usize>) {
                let length = self.first.len() + self.rest.len();
                (length, Some(length))
            }
        }

        impl$(<$lifetime>)? DoubleEndedIterator for $type$(<$lifetime>)? {
            fn next_back(&mut self) -> Option<Self::Item> {
                self.rest.next_back().or_else(|| self.first.next_back())
            }
        }

        impl$(<$lifetime>)? ExactSizeIterator for $type$(<$lifetime>)? {}
        impl$(<$lifetime>)? std::iter::FusedIterator for $type$(<$lifetime>)? {}
    };
}

impl_encoded_iterator!(EncodedValuesIter<'a>, &'a FieldValue);
impl_encoded_iterator!(EncodedValuesIterMut<'a>, &'a mut FieldValue);
impl_encoded_iterator!(EncodedValuesIntoIter, FieldValue);

macro_rules! impl_redacted_debug {
    ($type:ident $(<$lifetime:lifetime>)?) => {
        impl$(<$lifetime>)? fmt::Debug for $type$(<$lifetime>)? {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.debug_struct(stringify!($type))
                    .field("remaining", &self.len())
                    .finish()
            }
        }
    };
}

impl_redacted_debug!(EncodedValuesIter<'a>);
impl_redacted_debug!(EncodedValuesIterMut<'a>);
impl_redacted_debug!(EncodedValuesIntoIter);

impl Extend<FieldValue> for EncodedValues {
    fn extend<T>(&mut self, iter: T)
    where
        T: IntoIterator<Item = FieldValue>,
    {
        let mut iter = iter.into_iter();
        if self.is_empty() {
            self.first = iter.next();
        }
        self.rest.reserve(iter.size_hint().0);
        self.rest.extend(iter);
    }
}

impl FromIterator<FieldValue> for EncodedValues {
    fn from_iter<T: IntoIterator<Item = FieldValue>>(iter: T) -> Self {
        let mut values = Self::new();
        values.extend(iter);
        values
    }
}

impl From<FieldValue> for EncodedValues {
    fn from(value: FieldValue) -> Self {
        Self::single(value)
    }
}

impl IntoIterator for EncodedValues {
    type Item = FieldValue;
    type IntoIter = EncodedValuesIntoIter;

    fn into_iter(self) -> Self::IntoIter {
        EncodedValuesIntoIter {
            first: self.first.into_iter(),
            rest: self.rest.into_iter(),
        }
    }
}

impl<'a> IntoIterator for &'a EncodedValues {
    type Item = &'a FieldValue;
    type IntoIter = EncodedValuesIter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<'a> IntoIterator for &'a mut EncodedValues {
    type Item = &'a mut FieldValue;
    type IntoIter = EncodedValuesIterMut<'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter_mut()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::cell::Cell;
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    use super::EncodedValues;
    use crate::FieldValue;

    struct HintingValues<'a> {
        remaining: usize,
        hint_calls: &'a Cell<usize>,
    }

    impl Iterator for HintingValues<'_> {
        type Item = FieldValue;

        fn next(&mut self) -> Option<Self::Item> {
            if self.remaining == 0 {
                return None;
            }
            self.remaining -= 1;
            Some(FieldValue::from_static("reserved"))
        }

        fn size_hint(&self) -> (usize, Option<usize>) {
            self.hint_calls.set(self.hint_calls.get() + 1);
            (self.remaining, Some(self.remaining))
        }
    }

    #[test]
    fn collection_and_iterators_preserve_values_and_redact_debug_output() {
        let mut values = EncodedValues::from_vec(vec![FieldValue::from_static("first"), FieldValue::from_static("second")]);
        assert_eq!(format!("{values:?}"), "EncodedValues { value_count: 2 }");

        let mut iter = values.iter();
        assert_eq!(iter.size_hint(), (2, Some(2)));
        assert_eq!(format!("{iter:?}"), "EncodedValuesIter { remaining: 2 }");
        assert_eq!(iter.next().expect("first value"), "first");
        assert_eq!(iter.next().expect("second value"), "second");
        assert!(iter.next().is_none());

        let mut iter = values.iter_mut();
        assert_eq!(iter.size_hint(), (2, Some(2)));
        assert_eq!(format!("{iter:?}"), "EncodedValuesIterMut { remaining: 2 }");
        iter.next().expect("first mutable value").set_sensitive(true);
        assert!(values.iter().next().expect("first value").is_sensitive());

        let mut iter = values.into_iter();
        assert_eq!(iter.size_hint(), (2, Some(2)));
        assert_eq!(format!("{iter:?}"), "EncodedValuesIntoIter { remaining: 2 }");
        assert_eq!(iter.next().expect("first owned value"), "first");
        assert_eq!(iter.next().expect("second owned value"), "second");
        assert!(iter.next().is_none());
    }

    #[test]
    fn iterators_support_mixed_direction_iteration() {
        let values = ["first", "second", "third"]
            .into_iter()
            .map(FieldValue::from_static)
            .collect::<EncodedValues>();

        let mut borrowed = values.iter();
        assert_eq!(borrowed.next_back().expect("last value"), "third");
        assert_eq!(borrowed.next().expect("first value"), "first");
        assert_eq!(borrowed.next_back().expect("middle value"), "second");
        assert!(borrowed.next().is_none());

        let mut owned = values.into_iter();
        assert_eq!(owned.next_back().expect("last owned value"), "third");
        assert_eq!(owned.next().expect("first owned value"), "first");
        assert_eq!(owned.next_back().expect("middle owned value"), "second");
        assert!(owned.next().is_none());
    }

    #[test]
    fn comparison_and_hashing_ignore_internal_representation() {
        let single = EncodedValues::single(FieldValue::from_static("gzip"));
        let vector = EncodedValues::from_vec(vec![FieldValue::from_static("gzip")]);
        let greater = EncodedValues::from_vec(vec![FieldValue::from_static("gzip, br")]);

        assert_eq!(single, vector);
        assert!(single < greater);

        let mut single_hash = DefaultHasher::new();
        single.hash(&mut single_hash);
        let mut vector_hash = DefaultHasher::new();
        vector.hash(&mut vector_hash);
        assert_eq!(single_hash.finish(), vector_hash.finish());
    }

    #[test]
    fn construction_extension_and_borrowed_iteration_cover_all_storage_shapes() {
        let mut values = EncodedValues::new();
        assert!(values.is_empty());
        values.push(FieldValue::from_static("first"));
        assert!(!values.is_empty());
        values.extend([FieldValue::from_static("second"), FieldValue::from_static("third")]);
        assert_eq!(values.len(), 3);
        assert_eq!(
            (&values).into_iter().map(FieldValue::as_bytes).collect::<Vec<_>>(),
            [b"first".as_slice(), b"second".as_slice(), b"third".as_slice()]
        );

        let hint_calls = Cell::new(0);
        let reserved = HintingValues {
            remaining: 16,
            hint_calls: &hint_calls,
        }
        .collect::<EncodedValues>();
        assert_eq!(reserved.len(), 16);
        assert_ne!(hint_calls.get(), 0);

        for value in &mut values {
            value.set_sensitive(true);
        }
        assert!(values.iter().all(FieldValue::is_sensitive));

        let one = EncodedValues::from(FieldValue::from_static("one"));
        assert_eq!(one.len(), 1);
        let collected: EncodedValues = [FieldValue::from_static("a"), FieldValue::from_static("b")].into_iter().collect();
        assert_eq!(collected.len(), 2);
    }
}
