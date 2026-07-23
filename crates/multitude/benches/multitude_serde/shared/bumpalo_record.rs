// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use core::fmt;

use bumpalo::Bump;
use bumpalo::collections::Vec;
use serde::Deserialize;
use serde::de::{self, DeserializeSeed, Deserializer, MapAccess, SeqAccess, Visitor};

pub(super) struct BumpRecord<'a> {
    id: u64,
    name: &'a str,
    tags: Vec<'a, &'a str>,
    metadata: BumpMetadata<'a>,
}

impl BumpRecord<'_> {
    pub(super) fn summary(&self) -> (u64, usize, usize, bool, usize) {
        (
            self.id,
            self.name.len(),
            self.tags.iter().map(|tag| tag.len()).sum(),
            self.metadata.active,
            self.metadata.description.len(),
        )
    }
}

struct BumpMetadata<'a> {
    active: bool,
    description: &'a str,
}

struct StringSeed<'a>(&'a Bump);

impl<'de, 'a> DeserializeSeed<'de> for StringSeed<'a> {
    type Value = &'a str;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        struct StringVisitor<'a>(&'a Bump);

        impl<'de, 'a> Visitor<'de> for StringVisitor<'a> {
            type Value = &'a str;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a string")
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
                Ok(self.0.alloc_str(v))
            }

            fn visit_borrowed_str<E: de::Error>(self, v: &'de str) -> Result<Self::Value, E> {
                self.visit_str(v)
            }

            fn visit_string<E: de::Error>(self, v: String) -> Result<Self::Value, E> {
                self.visit_str(&v)
            }
        }

        deserializer.deserialize_str(StringVisitor(self.0))
    }
}

struct TagsSeed<'a>(&'a Bump);

impl<'de, 'a> DeserializeSeed<'de> for TagsSeed<'a> {
    type Value = Vec<'a, &'a str>;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        struct TagsVisitor<'a>(&'a Bump);

        impl<'de, 'a> Visitor<'de> for TagsVisitor<'a> {
            type Value = Vec<'a, &'a str>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a sequence of strings")
            }

            fn visit_seq<S: SeqAccess<'de>>(self, mut seq: S) -> Result<Self::Value, S::Error> {
                let mut tags = Vec::with_capacity_in(seq.size_hint().unwrap_or(0), self.0);
                while let Some(tag) = seq.next_element_seed(StringSeed(self.0))? {
                    tags.push(tag);
                }
                Ok(tags)
            }
        }

        deserializer.deserialize_seq(TagsVisitor(self.0))
    }
}

#[derive(Deserialize)]
#[serde(field_identifier, rename_all = "lowercase")]
enum MetadataField {
    Active,
    Description,
    #[serde(other)]
    Ignore,
}

struct MetadataSeed<'a>(&'a Bump);

impl<'de, 'a> DeserializeSeed<'de> for MetadataSeed<'a> {
    type Value = BumpMetadata<'a>;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        struct MetadataVisitor<'a>(&'a Bump);

        impl<'de, 'a> Visitor<'de> for MetadataVisitor<'a> {
            type Value = BumpMetadata<'a>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("metadata")
            }

            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let mut active = None;
                let mut description = None;
                while let Some(field) = map.next_key::<MetadataField>()? {
                    match field {
                        MetadataField::Active => {
                            if active.replace(map.next_value()?).is_some() {
                                return Err(de::Error::duplicate_field("active"));
                            }
                        }
                        MetadataField::Description => {
                            if description.replace(map.next_value_seed(StringSeed(self.0))?).is_some() {
                                return Err(de::Error::duplicate_field("description"));
                            }
                        }
                        MetadataField::Ignore => {
                            map.next_value::<de::IgnoredAny>()?;
                        }
                    }
                }
                Ok(BumpMetadata {
                    active: active.ok_or_else(|| de::Error::missing_field("active"))?,
                    description: description.ok_or_else(|| de::Error::missing_field("description"))?,
                })
            }
        }

        deserializer.deserialize_struct("BumpMetadata", &["active", "description"], MetadataVisitor(self.0))
    }
}

#[derive(Deserialize)]
#[serde(field_identifier, rename_all = "lowercase")]
enum RecordField {
    Id,
    Name,
    Tags,
    Metadata,
    #[serde(other)]
    Ignore,
}

pub(super) struct BumpRecordSeed<'a>(pub(super) &'a Bump);

impl<'de, 'a> DeserializeSeed<'de> for BumpRecordSeed<'a> {
    type Value = BumpRecord<'a>;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        struct RecordVisitor<'a>(&'a Bump);

        impl<'de, 'a> Visitor<'de> for RecordVisitor<'a> {
            type Value = BumpRecord<'a>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a record")
            }

            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let mut id = None;
                let mut name = None;
                let mut tags = None;
                let mut metadata = None;
                while let Some(field) = map.next_key::<RecordField>()? {
                    match field {
                        RecordField::Id => {
                            if id.replace(map.next_value()?).is_some() {
                                return Err(de::Error::duplicate_field("id"));
                            }
                        }
                        RecordField::Name => {
                            if name.replace(map.next_value_seed(StringSeed(self.0))?).is_some() {
                                return Err(de::Error::duplicate_field("name"));
                            }
                        }
                        RecordField::Tags => {
                            if tags.replace(map.next_value_seed(TagsSeed(self.0))?).is_some() {
                                return Err(de::Error::duplicate_field("tags"));
                            }
                        }
                        RecordField::Metadata => {
                            if metadata.replace(map.next_value_seed(MetadataSeed(self.0))?).is_some() {
                                return Err(de::Error::duplicate_field("metadata"));
                            }
                        }
                        RecordField::Ignore => {
                            map.next_value::<de::IgnoredAny>()?;
                        }
                    }
                }
                Ok(BumpRecord {
                    id: id.ok_or_else(|| de::Error::missing_field("id"))?,
                    name: name.ok_or_else(|| de::Error::missing_field("name"))?,
                    tags: tags.ok_or_else(|| de::Error::missing_field("tags"))?,
                    metadata: metadata.ok_or_else(|| de::Error::missing_field("metadata"))?,
                })
            }
        }

        deserializer.deserialize_struct("BumpRecord", &["id", "name", "tags", "metadata"], RecordVisitor(self.0))
    }
}
