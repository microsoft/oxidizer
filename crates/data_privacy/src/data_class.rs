// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use core::fmt::Display;
use std::borrow::Cow;

/// The identity of a well-known data class.
///
/// Each data class has a name, which is unique in the context of a specific named taxonomy.
#[derive(Debug, Clone, Hash, Eq, PartialEq, Ord, PartialOrd)]
pub struct DataClass {
    taxonomy: Cow<'static, str>,
    name: Cow<'static, str>,
}

impl AsRef<Self> for DataClass {
    fn as_ref(&self) -> &Self {
        self
    }
}

impl DataClass {
    /// Creates a new data class instance.
    #[must_use]
    pub const fn new(taxonomy: &'static str, name: &'static str) -> Self {
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
mod serde_impl {
    use super::DataClass;
    use serde_core::de::{self, Deserializer, MapAccess, Visitor};
    use serde_core::ser::{Serializer, SerializeStruct};
    use serde_core::{Deserialize, Serialize};
    use std::borrow::Cow;
    use core::fmt;

    impl Serialize for DataClass {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            let mut state = serializer.serialize_struct("DataClass", 2)?;
            state.serialize_field("taxonomy", &self.taxonomy)?;
            state.serialize_field("name", &self.name)?;
            state.end()
        }
    }

    impl<'de> Deserialize<'de> for DataClass {
        fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: Deserializer<'de>,
        {
            enum Field {
                Taxonomy,
                Name,
            }

            impl<'de> Deserialize<'de> for Field {
                fn deserialize<D>(deserializer: D) -> Result<Field, D::Error>
                where
                    D: Deserializer<'de>,
                {
                    struct FieldVisitor;

                    impl Visitor<'_> for FieldVisitor {
                        type Value = Field;

                        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                            formatter.write_str("`taxonomy` or `name`")
                        }

                        fn visit_str<E>(self, value: &str) -> Result<Field, E>
                        where
                            E: de::Error,
                        {
                            match value {
                                "taxonomy" => Ok(Field::Taxonomy),
                                "name" => Ok(Field::Name),
                                _ => Err(de::Error::unknown_field(value, &["taxonomy", "name"])),
                            }
                        }
                    }

                    deserializer.deserialize_identifier(FieldVisitor)
                }
            }

            struct DataClassVisitor;

            impl<'de> Visitor<'de> for DataClassVisitor {
                type Value = DataClass;

                fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                    formatter.write_str("struct DataClass")
                }

                fn visit_map<V>(self, mut map: V) -> Result<DataClass, V::Error>
                where
                    V: MapAccess<'de>,
                {
                    let mut taxonomy: Option<String> = None;
                    let mut name: Option<String> = None;

                    while let Some(key) = map.next_key()? {
                        match key {
                            Field::Taxonomy => {
                                if taxonomy.is_some() {
                                    return Err(de::Error::duplicate_field("taxonomy"));
                                }
                                taxonomy = Some(map.next_value()?);
                            }
                            Field::Name => {
                                if name.is_some() {
                                    return Err(de::Error::duplicate_field("name"));
                                }
                                name = Some(map.next_value()?);
                            }
                        }
                    }

                    let taxonomy = taxonomy.ok_or_else(|| de::Error::missing_field("taxonomy"))?;
                    let name = name.ok_or_else(|| de::Error::missing_field("name"))?;

                    Ok(DataClass {
                        taxonomy: Cow::Owned(taxonomy),
                        name: Cow::Owned(name),
                    })
                }
            }

            deserializer.deserialize_struct("DataClass", &["taxonomy", "name"], DataClassVisitor)
        }
    }
}
