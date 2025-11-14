// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::Debug;
use std::hash::{Hash, Hasher};

use crate::common_taxonomy::CommonTaxonomy;
use crate::{Classified, DataClass};

/// A wrapper type that classifies a value with a specific data class.
///
/// Use this wrapper in places where [`DataClass`] can be set dynamically, and where it's preferable to avoid
/// generics in form of `impl Classified`
///
/// If possible, use the specific classification types like [`Sensitive`](crate::common_taxonomy::Sensitive) instead.
pub struct ClassifiedWrapper<T> {
    value: T,
    data_class: DataClass,
}

impl<T> ClassifiedWrapper<T> {
    /// Creates a new instance of `ClassifiedWrapper` with the given value and data class.
    pub const fn new(value: T, data_class: DataClass) -> Self {
        Self { value, data_class }
    }
}

impl<T> Debug for ClassifiedWrapper<T>
where
    T: Debug,
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_fmt(format_args!(
            "<CLASSIFIED:{}/{}>",
            self.data_class.taxonomy(),
            self.data_class.name()
        ))
    }
}

impl<T> Classified for ClassifiedWrapper<T> {
    type Payload = T;

    fn declassify(self) -> T {
        self.value
    }

    fn as_declassified(&self) -> &T {
        &self.value
    }

    fn as_declassified_mut(&mut self) -> &mut T {
        &mut self.value
    }

    fn data_class(&self) -> DataClass {
        self.data_class.clone()
    }
}

impl<T> From<T> for ClassifiedWrapper<T> {
    fn from(value: T) -> Self {
        Self {
            value,
            data_class: CommonTaxonomy::UnknownSensitivity.data_class(),
        }
    }
}

impl<T> Clone for ClassifiedWrapper<T>
where
    T: Clone,
{
    fn clone(&self) -> Self {
        Self {
            value: self.value.clone(),
            data_class: self.data_class.clone(),
        }
    }
}

/// Compares values of two `ClassifiedWrapper` instances for equality.
impl<T> PartialEq for ClassifiedWrapper<T>
where
    T: PartialEq,
{
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
    }
}

impl<T> Hash for ClassifiedWrapper<T>
where
    T: Hash,
{
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.value.hash(state);
    }
}

/// Implements `PartialOrd` trait for `ClassifiedWrapper`.
/// this compares only the value, ignoring the data class.
impl<T> PartialOrd for ClassifiedWrapper<T>
where
    T: PartialOrd,
{
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        self.value.partial_cmp(&other.value)
    }
}

#[cfg(test)]
mod tests {
    use std::cmp::Ordering;

    use super::*;

    #[test]
    fn test_classified_wrapper() {
        let classified = ClassifiedWrapper::new(42, CommonTaxonomy::Sensitive.data_class());
        assert_eq!(classified.as_declassified(), &42);
        assert_eq!(classified.data_class(), CommonTaxonomy::Sensitive.data_class());
        assert_eq!(format!("{classified:?}"), "<CLASSIFIED:common/sensitive>");
    }

    #[test]
    fn test_clone_and_equality() {
        let classified1 = ClassifiedWrapper::new(42, CommonTaxonomy::Sensitive.data_class());
        let classified2 = classified1.clone();
        let classified3 = ClassifiedWrapper::new(12, CommonTaxonomy::Sensitive.data_class());
        assert_eq!(classified1, classified2);
        assert_ne!(classified1, classified3);
    }

    #[test]
    fn test_hash() {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        let classified = ClassifiedWrapper::new(42, CommonTaxonomy::Sensitive.data_class());
        classified.hash(&mut hasher);
        let hash1 = hasher.finish();

        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        let classified2 = ClassifiedWrapper::new(42, CommonTaxonomy::Sensitive.data_class());
        classified2.hash(&mut hasher);
        let hash2 = hasher.finish();

        assert_eq!(hash1, hash2, "Hashes should be equal for the same classified value");

        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        let classified3 = ClassifiedWrapper::new(12, CommonTaxonomy::Sensitive.data_class());
        classified3.hash(&mut hasher);
        let hash3 = hasher.finish();

        assert_ne!(hash1, hash3, "Hashes of data with different values should not be equal");
    }

    #[test]
    fn test_ordering() {
        let classified1 = ClassifiedWrapper::new(42, CommonTaxonomy::Sensitive.data_class());
        let classified2 = ClassifiedWrapper::new(12, CommonTaxonomy::Sensitive.data_class());

        assert_eq!(classified1.partial_cmp(&classified2).unwrap(), Ordering::Greater);
        assert_eq!(classified2.partial_cmp(&classified1).unwrap(), Ordering::Less);
        assert_eq!(classified1.partial_cmp(&classified1).unwrap(), Ordering::Equal);
    }

    #[test]
    fn test_declassify_returns_inner_value() {
        // Consuming declassification returns the inner value
        let classified = ClassifiedWrapper::new(String::from("secret"), CommonTaxonomy::Sensitive.data_class());
        let value = classified.declassify();
        assert_eq!(value, "secret");
    }

    #[test]
    fn test_as_declassified_mut_allows_mutation() {
        // Mutable access allows in-place mutation of the wrapped value
        let mut classified = ClassifiedWrapper::new(vec![1, 2, 3], CommonTaxonomy::Sensitive.data_class());
        classified.as_declassified_mut().push(4);
        assert_eq!(classified.as_declassified(), &vec![1, 2, 3, 4]);
        // Ensure the data class remains unchanged after mutation
        assert_eq!(classified.data_class(), CommonTaxonomy::Sensitive.data_class());
    }

    #[test]
    fn test_from_impl_sets_unknown_sensitivity() {
        // Using From<T> should default to UnknownSensitivity
        let classified: ClassifiedWrapper<String> = String::from("hello").into();
        assert_eq!(classified.as_declassified(), &"hello".to_string());
        assert_eq!(classified.data_class(), CommonTaxonomy::UnknownSensitivity.data_class());
        // Debug should redact and include the correct class path
        assert_eq!(format!("{classified:?}"), "<CLASSIFIED:common/unknown_sensitivity>");
    }
}
