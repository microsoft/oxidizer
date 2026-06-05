// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use core::fmt::Display;
use std::borrow::Cow;

#[cfg(feature = "serde")]
use serde_core::{Deserialize, Deserializer, Serialize, Serializer, de};

/// The identity of a well-known data class.
///
/// Each data class has a name, which is unique in the context of a specific named taxonomy.
///
/// # Serialization
///
/// Serializing a `DataClass` produces a string in the format `taxonomy/name`.
#[derive(Debug, Clone, Hash, Eq, PartialEq, Ord, PartialOrd)]
pub struct DataClass {
    taxonomy: Cow<'static, str>,
    name: Cow<'static, str>,
}

impl DataClass {
    /// Creates a new data class instance.
    ///
    /// # Panics
    ///
    /// Panics if `taxonomy` or `name` are not valid identifiers. Valid identifiers must
    /// start with `_` or an ASCII letter, followed by zero or more `_`, ASCII letters, or ASCII
    /// digits (e.g., `foo`, `_bar`, `Baz123`)
    #[must_use]
    pub const fn new(taxonomy: &'static str, name: &'static str) -> Self {
        assert!(is_valid_identifier(taxonomy), "taxonomy is not a valid identifier");
        assert!(is_valid_identifier(name), "name is not a valid identifier");

        Self {
            taxonomy: Cow::Borrowed(taxonomy),
            name: Cow::Borrowed(name),
        }
    }

    /// Returns the taxonomy of the data class.
    #[must_use]
    pub fn taxonomy(&self) -> &str {
        &self.taxonomy
    }

    /// Returns the name of the data class.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }
}

impl Display for DataClass {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}/{}", self.taxonomy, self.name)
    }
}

impl AsRef<Self> for DataClass {
    fn as_ref(&self) -> &Self {
        self
    }
}

/// Helper for converting a type into a [`DataClass`].
pub trait IntoDataClass {
    /// Converts `self` into a [`DataClass`].
    fn into_data_class(self) -> DataClass;
}

impl IntoDataClass for DataClass {
    fn into_data_class(self) -> DataClass {
        self
    }
}

#[cfg(feature = "serde")]
impl Serialize for DataClass {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

#[cfg(feature = "serde")]
impl<'de> Deserialize<'de> for DataClass {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct DataClassVisitor;

        impl de::Visitor<'_> for DataClassVisitor {
            type Value = DataClass;

            fn expecting(&self, formatter: &mut core::fmt::Formatter) -> core::fmt::Result {
                formatter.write_str("a string in taxonomy/name format")
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
                let (taxonomy, name) = v
                    .split_once('/')
                    .ok_or_else(|| de::Error::custom("expecting taxonomy/name format"))?;

                if !is_valid_identifier(taxonomy) {
                    return Err(de::Error::custom("invalid taxonomy identifier"));
                }

                if !is_valid_identifier(name) {
                    return Err(de::Error::custom("invalid name identifier"));
                }

                Ok(DataClass {
                    taxonomy: Cow::Owned(taxonomy.to_owned()),
                    name: Cow::Owned(name.to_owned()),
                })
            }
        }

        deserializer.deserialize_str(DataClassVisitor)
    }
}

/// Checks if a byte is a valid ASCII start character for a Rust identifier.
const fn is_valid_ascii_ident_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_'
}

/// Checks if a byte is a valid ASCII continuation character for a Rust identifier.
const fn is_valid_ascii_ident_continue(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// Validates that a string is a valid Rust identifier (ASCII only).
///
/// This supports standard ASCII identifiers: `foo`, `_bar`, `Baz123`
///
/// Valid identifiers must:
/// - Start with `_` or an ASCII letter (a-z, A-Z)
/// - Continue with zero or more `_`, ASCII letters, or ASCII digits (0-9)
///
/// This function is `const` and can be used in both const and runtime contexts.
#[cfg_attr(test, mutants::skip)] // leads to build timeouts in mutant tests
const fn is_valid_identifier(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.is_empty() {
        return false;
    }

    if !is_valid_ascii_ident_start(bytes[0]) {
        return false;
    }

    let mut i = 1;
    while i < bytes.len() {
        if !is_valid_ascii_ident_continue(bytes[i]) {
            return false;
        }
        i += 1;
    }

    true
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn new_stores_taxonomy_and_name() {
        let dc = DataClass::new("contoso", "customer_id");
        assert_eq!(dc.taxonomy(), "contoso");
        assert_eq!(dc.name(), "customer_id");
    }

    #[test]
    fn display_uses_slash_separator() {
        let dc = DataClass::new("contoso", "customer_id");
        assert_eq!(dc.to_string(), "contoso/customer_id");
    }

    #[test]
    fn as_ref_returns_self() {
        let dc = DataClass::new("contoso", "customer_id");
        let dc_ref: &DataClass = dc.as_ref();
        assert_eq!(dc_ref, &dc);
    }

    #[test]
    fn into_data_class_identity() {
        let dc = DataClass::new("contoso", "customer_id");
        let dc2 = dc.clone().into_data_class();
        assert_eq!(dc, dc2);
    }

    #[test]
    fn valid_identifiers() {
        assert!(is_valid_identifier("foo"));
        assert!(is_valid_identifier("_bar"));
        assert!(is_valid_identifier("Baz123"));
        assert!(is_valid_identifier("_"));
        assert!(is_valid_identifier("a"));
    }

    #[test]
    fn invalid_identifiers() {
        assert!(!is_valid_identifier(""));
        assert!(!is_valid_identifier("1abc"));
        assert!(!is_valid_identifier("a-b"));
        assert!(!is_valid_identifier("a b"));
    }

    #[test]
    fn data_class_equality_and_ordering() {
        let a = DataClass::new("a", "x");
        let b = DataClass::new("b", "x");
        assert!(a < b);
        assert_eq!(a, a.clone());
    }

    #[test]
    fn data_class_debug() {
        let dc = DataClass::new("t", "n");
        let dbg = format!("{dc:?}");
        assert!(dbg.contains("DataClass"));
    }

    #[test]
    fn data_class_hash() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(DataClass::new("t", "n"));
        assert!(set.contains(&DataClass::new("t", "n")));
    }
}

#[cfg(all(test, feature = "serde"))]
#[cfg_attr(coverage_nightly, coverage(off))]
mod serde_tests {
    use super::*;

    #[test]
    fn test_serialize() {
        let dc = DataClass::new("contoso", "customer_identifier");
        let serialized = serde_json::to_string(&dc).expect("failed to serialize");
        assert_eq!(serialized, "\"contoso/customer_identifier\"");
    }

    #[test]
    fn test_deserialize_valid() {
        let serialized = "\"contoso/customer_identifier\"";
        let dc: DataClass = serde_json::from_str(serialized).expect("failed to deserialize");
        assert_eq!(dc.taxonomy(), "contoso");
        assert_eq!(dc.name(), "customer_identifier");
    }

    #[test]
    fn test_deserialize_invalid_format_no_slash() {
        let serialized = "\"contoso_customer_identifier\"";
        let err = serde_json::from_str::<DataClass>(serialized).unwrap_err();
        assert!(err.to_string().contains("expecting taxonomy/name format"));
    }

    #[test]
    fn test_deserialize_invalid_format_empty_taxonomy() {
        let serialized = "\"/customer_identifier\"";
        let err = serde_json::from_str::<DataClass>(serialized).unwrap_err();
        assert!(err.to_string().contains("invalid taxonomy identifier"));
    }

    #[test]
    fn test_deserialize_invalid_format_empty_name() {
        let serialized = "\"contoso/\"";
        let err = serde_json::from_str::<DataClass>(serialized).unwrap_err();
        assert!(err.to_string().contains("invalid name identifier"));
    }

    #[test]
    fn test_deserialize_invalid_taxonomy() {
        let serialized = "\"a-b/c\"";
        let err = serde_json::from_str::<DataClass>(serialized).unwrap_err();
        assert!(err.to_string().contains("invalid taxonomy identifier"));
    }

    #[test]
    fn test_deserialize_invalid_name() {
        let serialized = "\"a/b-c\"";
        let err = serde_json::from_str::<DataClass>(serialized).unwrap_err();
        assert!(err.to_string().contains("invalid name identifier"));
    }

    #[test]
    fn test_deserialize_wrong_type_triggers_expecting() {
        // Passing a number instead of a string triggers Visitor::expecting
        let err = serde_json::from_str::<DataClass>("42").unwrap_err();
        assert!(err.to_string().contains("a string in taxonomy/name format"));
    }
}
