// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Deserializes a JSON body with typed query-field overlays.
//!
//! Body values remain borrowed [`RawValue`](serde_json::value::RawValue)s.
//! Query values adapt to the scalar, sequence, or nested message type requested
//! by serde.

use core::fmt;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::vec::IntoIter;

use serde::de::value::{StrDeserializer, StringDeserializer};
use serde::de::{DeserializeOwned, DeserializeSeed, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::value::RawValue;
use serde_json::{Deserializer as JsonDeserializer, Error as JsonError, Number, Value, from_slice, from_str};
use smallvec::{SmallVec, smallvec};

use super::request_body_kind::RequestBodyKind;
use super::{TranscodeError, percent};

/// Decodes through the overlay when the direct field-body path does not apply.
pub(crate) fn try_decode_overlay<T: DeserializeOwned>(
    body_kind: &RequestBodyKind,
    query: &[(&str, &str)],
    body: &[u8],
) -> Option<Result<T, TranscodeError>> {
    let whole_body = match body_kind {
        RequestBodyKind::None => None,
        RequestBodyKind::Whole => Some(body),
        RequestBodyKind::Field(field) => {
            if query.is_empty() {
                return None;
            }
            let entries = if body.is_empty() {
                Vec::new()
            } else {
                let raw = match from_slice::<&RawValue>(body) {
                    Ok(raw) => raw,
                    Err(error) => return Some(Err(TranscodeError::body(error))),
                };
                vec![(field.to_string(), raw)]
            };
            return Some(decode_tree_entries(entries, query));
        }
    };

    if all_flat(query) && !query.iter().any(|(key, _)| percent::needs_decoding(key, true)) {
        Some(decode_flat(whole_body, query))
    } else {
        Some(decode_tree(whole_body, query))
    }
}

fn all_flat(query: &[(&str, &str)]) -> bool {
    query.iter().all(|(key, _)| !key.contains('.'))
}

/// Normalizes a proto3-JSON field key for identity comparison.
///
/// proto3 JSON accepts a field under both its `snake_case` proto name and its
/// `lowerCamelCase` JSON name, which denote the same field. Folding case and
/// dropping `_` maps both spellings to one key, so the overlay can treat them as
/// the same field when deduplicating body entries and applying query overrides.
fn normalized_key(key: &str) -> Cow<'_, str> {
    if key.bytes().all(|byte| byte != b'_' && !byte.is_ascii_uppercase()) {
        Cow::Borrowed(key)
    } else {
        Cow::Owned(key.chars().filter(|&ch| ch != '_').map(|ch| ch.to_ascii_lowercase()).collect())
    }
}

type QueryValues<'de> = SmallVec<[Cow<'de, str>; 2]>;
type FlatOverlay<'de> = SmallVec<[(&'de str, QueryValues<'de>); 8]>;
type FlatOverlayIndex<'de> = HashMap<&'de str, usize>;
type GroupedFlatQuery<'de> = (FlatOverlay<'de>, Option<FlatOverlayIndex<'de>>);

fn decode_flat<T: DeserializeOwned>(body: Option<&[u8]>, query: &[(&str, &str)]) -> Result<T, TranscodeError> {
    let (overlay, _overlay_index) = group_flat_query(query)?;

    let mut body_entries = match body {
        Some(bytes) if !bytes.is_empty() => match from_slice::<BodyTop<'_>>(bytes) {
            Ok(top) => top.entries,
            Err(_) => return Err(classify_body_error(bytes)),
        },
        _ => Vec::new(),
    };

    let query_norms: SmallVec<[Cow<'_, str>; 8]> = overlay.iter().map(|(key, _)| normalized_key(key)).collect();
    let query_norm_set: Option<HashSet<&str>> =
        (query_norms.len() > query_norms.inline_size()).then(|| query_norms.iter().map(Cow::as_ref).collect());
    body_entries.retain(|(key, _)| {
        let normalized = normalized_key(key);
        match &query_norm_set {
            Some(set) => !set.contains(normalized.as_ref()),
            None => !query_norms.contains(&normalized),
        }
    });

    let map = OverlayMap {
        body: body_entries.into_iter(),
        overlay: overlay.into_iter(),
        pending: None,
    };
    T::deserialize(OverlayDeserializer { map }).map_err(TranscodeError::deserialize)
}

fn group_flat_query<'de>(query: &[(&'de str, &'de str)]) -> Result<GroupedFlatQuery<'de>, TranscodeError> {
    let mut overlay = FlatOverlay::new();
    let mut overlay_index: Option<FlatOverlayIndex<'de>> = None;
    for (key, value) in query {
        let decoded = percent::decode_query(value).ok_or_else(|| TranscodeError::invalid_encoding("query parameter value"))?;
        let existing = match &overlay_index {
            Some(index) => index.get(key).copied(),
            None => overlay.iter().position(|(existing, _)| existing == key),
        };
        if let Some(index) = existing {
            overlay[index].1.push(decoded);
        } else {
            if overlay_index.is_none() && overlay.len() == overlay.inline_size() {
                let mut index = HashMap::with_capacity(query.len());
                index.extend(overlay.iter().enumerate().map(|(position, (existing, _))| (*existing, position)));
                overlay_index = Some(index);
            }
            if let Some(index) = &mut overlay_index {
                index.insert(*key, overlay.len());
            }
            overlay.push((key, smallvec![decoded]));
        }
    }
    Ok((overlay, overlay_index))
}

fn classify_body_error(bytes: &[u8]) -> TranscodeError {
    match from_slice::<Value>(bytes) {
        Err(source) => TranscodeError::body(source),
        // A valid JSON object that `BodyTop` rejects can only fail its duplicate-field check.
        Ok(Value::Object(_)) => TranscodeError::structure("request body contains a duplicate field"),
        Ok(_) => TranscodeError::structure("request body must be a JSON object"),
    }
}

/// Borrowed values from the body's top-level object.
struct BodyTop<'de> {
    entries: Vec<(String, &'de RawValue)>,
}

impl<'de> serde::Deserialize<'de> for BodyTop<'de> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct TopVisitor;

        impl<'de> Visitor<'de> for TopVisitor {
            type Value = BodyTop<'de>;

            #[cfg_attr(test, mutants::skip)]
            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a JSON object")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut entries: Vec<(String, &'de RawValue)> = Vec::new();
                let mut seen: HashMap<String, ()> = HashMap::new();
                while let Some(key) = map.next_key::<String>()? {
                    let value: &'de RawValue = map.next_value()?;
                    if seen.insert(normalized_key(&key).into_owned(), ()).is_some() {
                        return Err(serde::de::Error::custom("request body contains a duplicate field"));
                    }
                    entries.push((key, value));
                }
                Ok(BodyTop { entries })
            }
        }

        deserializer.deserialize_map(TopVisitor)
    }
}

enum Pending<'de> {
    Raw(&'de RawValue),
    Query(QueryValues<'de>),
}

struct OverlayMap<'de, B, O>
where
    B: Iterator<Item = (String, &'de RawValue)>,
    O: Iterator<Item = (&'de str, QueryValues<'de>)>,
{
    body: B,
    overlay: O,
    pending: Option<Pending<'de>>,
}

impl<'de, B, O> MapAccess<'de> for OverlayMap<'de, B, O>
where
    B: Iterator<Item = (String, &'de RawValue)>,
    O: Iterator<Item = (&'de str, QueryValues<'de>)>,
{
    type Error = JsonError;

    fn next_key_seed<K: DeserializeSeed<'de>>(&mut self, seed: K) -> Result<Option<K::Value>, Self::Error> {
        if let Some((key, value)) = self.body.next() {
            self.pending = Some(Pending::Raw(value));
            return seed.deserialize(StrDeserializer::<JsonError>::new(&key)).map(Some);
        }
        if let Some((key, value)) = self.overlay.next() {
            self.pending = Some(Pending::Query(value));
            return seed.deserialize(StrDeserializer::<JsonError>::new(key)).map(Some);
        }
        Ok(None)
    }

    #[cfg_attr(coverage_nightly, coverage(off))]
    fn next_value_seed<V: DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value, Self::Error> {
        match self.pending.take() {
            Some(Pending::Raw(raw)) => {
                let mut deserializer = JsonDeserializer::from_str(raw.get());
                seed.deserialize(&mut deserializer)
            }
            Some(Pending::Query(values)) => seed.deserialize(QueryValue { values }),
            None => Err(serde::de::Error::custom("value requested before key")),
        }
    }
}

struct OverlayDeserializer<'de, B, O>
where
    B: Iterator<Item = (String, &'de RawValue)>,
    O: Iterator<Item = (&'de str, QueryValues<'de>)>,
{
    map: OverlayMap<'de, B, O>,
}

impl<'de, B, O> Deserializer<'de> for OverlayDeserializer<'de, B, O>
where
    B: Iterator<Item = (String, &'de RawValue)>,
    O: Iterator<Item = (&'de str, QueryValues<'de>)>,
{
    type Error = JsonError;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        visitor.visit_map(self.map)
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
        bytes byte_buf option unit unit_struct newtype_struct seq tuple
        tuple_struct map struct enum identifier ignored_any
    }
}

struct QueryValue<'de> {
    values: QueryValues<'de>,
}

impl QueryValue<'_> {
    fn one(self) -> Result<String, JsonError> {
        if self.values.len() != 1 {
            return Err(serde::de::Error::custom(
                "a non-repeated query field cannot receive multiple values",
            ));
        }
        Ok(self.values.into_iter().next().expect("length checked above").into_owned())
    }
}

macro_rules! deserialize_query_string {
    ($($method:ident),+ $(,)?) => {
        $(
            fn $method<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
                StringDeserializer::<JsonError>::new(self.one()?).$method(visitor)
            }
        )+
    };
}

macro_rules! deserialize_query_integer {
    ($(($method:ident, $ty:ty, $visit:ident)),+ $(,)?) => {
        $(
            fn $method<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
                let value = self.one()?;
                let parsed = value.parse::<$ty>().map_err(serde::de::Error::custom)?;
                visitor.$visit(parsed)
            }
        )+
    };
}

fn looks_like_noncanonical_number(value: &str) -> bool {
    value
        .as_bytes()
        .first()
        .is_some_and(|first| first.is_ascii_digit() || matches!(first, b'+' | b'-' | b'.'))
        || matches!(
            value.to_ascii_lowercase().as_str(),
            "inf" | "infinity" | "nan" | "+inf" | "+infinity" | "-inf"
        )
}

impl<'de> Deserializer<'de> for QueryValue<'de> {
    type Error = JsonError;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        let value = self.one()?;
        if let Ok(number) = value.parse::<i64>() {
            visitor.visit_i64(number)
        } else if let Ok(number) = value.parse::<u64>() {
            visitor.visit_u64(number)
        } else if value == "NaN" {
            visitor.visit_f64(f64::NAN)
        } else if value == "Infinity" {
            visitor.visit_f64(f64::INFINITY)
        } else if value == "-Infinity" {
            visitor.visit_f64(f64::NEG_INFINITY)
        } else if let Ok(number) = from_str::<Number>(&value) {
            visitor.visit_f64(number.as_f64().ok_or_else(|| serde::de::Error::custom("number is out of range"))?)
        } else if looks_like_noncanonical_number(&value) {
            Err(serde::de::Error::custom("query number is not in canonical protobuf JSON form"))
        } else {
            visitor.visit_string(value)
        }
    }

    fn deserialize_bool<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match self.one()?.as_str() {
            "true" => visitor.visit_bool(true),
            "false" => visitor.visit_bool(false),
            value => Err(serde::de::Error::invalid_value(serde::de::Unexpected::Str(value), &"true or false")),
        }
    }

    fn deserialize_str<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        visitor.visit_string(self.one()?)
    }

    fn deserialize_string<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        visitor.visit_string(self.one()?)
    }

    fn deserialize_f32<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        deserialize_query_float(&self.one()?, visitor, str::parse::<f32>)
    }

    fn deserialize_f64<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        deserialize_query_float(&self.one()?, visitor, str::parse::<f64>)
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        visitor.visit_some(self)
    }

    fn deserialize_seq<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        visitor.visit_seq(QuerySequence {
            values: self.values.into_iter(),
        })
    }

    deserialize_query_integer!(
        (deserialize_i8, i8, visit_i8),
        (deserialize_i16, i16, visit_i16),
        (deserialize_i32, i32, visit_i32),
        (deserialize_i64, i64, visit_i64),
        (deserialize_i128, i128, visit_i128),
        (deserialize_u8, u8, visit_u8),
        (deserialize_u16, u16, visit_u16),
        (deserialize_u32, u32, visit_u32),
        (deserialize_u64, u64, visit_u64),
        (deserialize_u128, u128, visit_u128),
    );
    deserialize_query_string!(
        deserialize_char,
        deserialize_bytes,
        deserialize_byte_buf,
        deserialize_unit,
        deserialize_identifier
    );

    serde::forward_to_deserialize_any! {
        unit_struct newtype_struct tuple tuple_struct map struct enum ignored_any
    }
}

fn deserialize_query_float<'de, T, V>(
    value: &str,
    visitor: V,
    parse: impl FnOnce(&str) -> Result<T, <T as core::str::FromStr>::Err>,
) -> Result<V::Value, JsonError>
where
    T: core::str::FromStr + Into<f64>,
    T::Err: core::fmt::Display,
    V: Visitor<'de>,
{
    let number = match value {
        "NaN" => f64::NAN,
        "Infinity" => f64::INFINITY,
        "-Infinity" => f64::NEG_INFINITY,
        _ => {
            from_str::<Number>(value)?;
            let parsed = parse(value).map_err(serde::de::Error::custom)?.into();
            if !parsed.is_finite() {
                return Err(serde::de::Error::custom("float is out of range"));
            }
            parsed
        }
    };
    visitor.visit_f64(number)
}

struct QuerySequence<'de> {
    values: smallvec::IntoIter<[Cow<'de, str>; 2]>,
}

impl<'de> SeqAccess<'de> for QuerySequence<'de> {
    type Error = JsonError;

    fn next_element_seed<T: DeserializeSeed<'de>>(&mut self, seed: T) -> Result<Option<T::Value>, Self::Error> {
        self.values
            .next()
            .map(|value| seed.deserialize(QueryValue { values: smallvec![value] }))
            .transpose()
    }
}

enum QueryNode {
    Map(Vec<(String, Self)>),
    Leaf(SmallVec<[String; 2]>),
}

fn decode_tree<T: DeserializeOwned>(body: Option<&[u8]>, query: &[(&str, &str)]) -> Result<T, TranscodeError> {
    let body = match body {
        Some(bytes) if !bytes.is_empty() => match from_slice::<BodyTop<'_>>(bytes) {
            Ok(top) => top.entries,
            Err(_) => return Err(classify_body_error(bytes)),
        },
        _ => Vec::new(),
    };
    decode_tree_entries(body, query)
}

fn decode_tree_entries<T: DeserializeOwned>(body: Vec<(String, &RawValue)>, query: &[(&str, &str)]) -> Result<T, TranscodeError> {
    let mut query_root = Vec::new();
    for (key, value) in query {
        let key = percent::decode_query(key).ok_or_else(|| TranscodeError::invalid_encoding("query parameter name"))?;
        let value = percent::decode_query(value).ok_or_else(|| TranscodeError::invalid_encoding("query parameter value"))?;
        insert_query_node(&mut query_root, key.split('.'), value.into_owned())?;
    }
    T::deserialize(TreeDeserializer { body, query: query_root }).map_err(TranscodeError::deserialize)
}

fn insert_query_node<'a>(
    map: &mut Vec<(String, QueryNode)>,
    mut path: impl Iterator<Item = &'a str> + Clone,
    value: String,
) -> Result<(), TranscodeError> {
    let key = path
        .next()
        .ok_or_else(|| TranscodeError::structure("query parameter has an empty field path"))?;
    let has_more = path.clone().next().is_some();
    let node = if let Some((_, node)) = map.iter_mut().find(|(existing, _)| existing == key) {
        node
    } else {
        map.push((
            key.to_owned(),
            if has_more {
                QueryNode::Map(Vec::new())
            } else {
                QueryNode::Leaf(SmallVec::new())
            },
        ));
        &mut map.last_mut().expect("entry was pushed immediately above").1
    };
    if has_more {
        match node {
            QueryNode::Map(children) => insert_query_node(children, path, value)?,
            QueryNode::Leaf(_) => return Err(TranscodeError::structure("query field conflicts with a nested field")),
        }
    } else {
        match node {
            QueryNode::Leaf(values) => values.push(value),
            QueryNode::Map(_) => return Err(TranscodeError::structure("query field conflicts with a nested field")),
        }
    }
    Ok(())
}

struct TreeDeserializer<'de> {
    body: Vec<(String, &'de RawValue)>,
    query: Vec<(String, QueryNode)>,
}

impl<'de> Deserializer<'de> for TreeDeserializer<'de> {
    type Error = JsonError;

    fn deserialize_any<V: Visitor<'de>>(mut self, visitor: V) -> Result<V::Value, Self::Error> {
        let mut query = Vec::with_capacity(self.query.len());
        for (key, node) in self.query {
            let normalized = normalized_key(&key);
            let body = self
                .body
                .iter()
                .position(|(body_key, _)| normalized_key(body_key) == normalized)
                .map(|index| self.body.remove(index).1);
            query.push((key, node, body));
        }
        visitor.visit_map(TreeMap {
            body: self.body.into_iter(),
            query: query.into_iter(),
            pending: None,
        })
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        visitor.visit_some(self)
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
        bytes byte_buf unit unit_struct newtype_struct seq tuple
        tuple_struct map struct enum identifier ignored_any
    }
}

enum TreePending<'de> {
    Raw(&'de RawValue),
    Node(QueryNode, Option<&'de RawValue>),
}

struct TreeMap<'de> {
    body: IntoIter<(String, &'de RawValue)>,
    query: IntoIter<(String, QueryNode, Option<&'de RawValue>)>,
    pending: Option<TreePending<'de>>,
}

impl<'de> MapAccess<'de> for TreeMap<'de> {
    type Error = JsonError;

    fn next_key_seed<K: DeserializeSeed<'de>>(&mut self, seed: K) -> Result<Option<K::Value>, Self::Error> {
        if let Some((key, value)) = self.body.next() {
            self.pending = Some(TreePending::Raw(value));
            return seed.deserialize(StrDeserializer::<JsonError>::new(&key)).map(Some);
        }
        if let Some((key, node, body)) = self.query.next() {
            self.pending = Some(TreePending::Node(node, body));
            return seed.deserialize(StrDeserializer::<JsonError>::new(&key)).map(Some);
        }
        Ok(None)
    }

    fn next_value_seed<V: DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value, Self::Error> {
        match self.pending.take() {
            Some(TreePending::Raw(raw)) => {
                let mut deserializer = JsonDeserializer::from_str(raw.get());
                seed.deserialize(&mut deserializer)
            }
            Some(TreePending::Node(QueryNode::Leaf(values), _)) => seed.deserialize(QueryValue {
                values: values.into_iter().map(Cow::Owned).collect(),
            }),
            Some(TreePending::Node(QueryNode::Map(query), body)) => {
                let body = match body {
                    Some(raw) => from_str::<BodyTop<'_>>(raw.get()).map_err(serde::de::Error::custom)?.entries,
                    None => Vec::new(),
                };
                seed.deserialize(TreeDeserializer { body, query })
            }
            None => Err(serde::de::Error::custom("value requested before key")),
        }
    }
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::*;

    #[derive(Debug, Deserialize, PartialEq)]
    struct Shelf {
        shelf: String,
        theme: String,
    }

    #[test]
    fn all_flat_is_true_only_for_top_level_targets() {
        assert!(all_flat(&[("theme", "history")]));
        assert!(!all_flat(&[("shelf.id", "7")]));
    }

    #[test]
    fn normalized_key_borrows_plain_keys_and_folds_snake_and_camel() {
        assert!(matches!(normalized_key("theme"), Cow::Borrowed("theme")));
        assert!(matches!(normalized_key("shelf_name"), Cow::Owned(owned) if owned == "shelfname"));
        assert!(matches!(normalized_key("shelfName"), Cow::Owned(owned) if owned == "shelfname"));
    }

    #[test]
    fn normalized_key_folds_ascii_case_without_garbling_multibyte_chars() {
        assert!(matches!(normalized_key("Café_Bar"), Cow::Owned(owned) if owned == "cafébar"));
    }

    #[test]
    fn large_query_deduplicates_body_entries_through_a_set() {
        #[derive(Debug, Deserialize, PartialEq)]
        struct Nine {
            a: String,
            b: String,
            c: String,
            d: String,
            e: String,
            f: String,
            g: String,
            h: String,
            i: String,
        }

        let query = [
            ("a", "qa"),
            ("b", "qb"),
            ("c", "qc"),
            ("d", "qd"),
            ("e", "qe"),
            ("f", "qf"),
            ("g", "qg"),
            ("h", "qh"),
            ("i", "qi"),
        ];
        let decoded: Nine = decode_flat(Some(br#"{"a":"body"}"#), &query).expect("decodes");
        assert_eq!(decoded.a, "qa");
        assert_eq!(decoded.i, "qi");
    }

    #[test]
    fn small_flat_queries_keep_grouping_storage_inline() {
        let query = [
            ("a", "1"),
            ("b", "2"),
            ("c", "3"),
            ("d", "4"),
            ("e", "5"),
            ("f", "6"),
            ("g", "7"),
            ("a", "8"),
        ];
        let (overlay, index) = group_flat_query(&query).expect("query groups");
        assert!(!overlay.spilled());
        assert!(index.is_none());
        assert!(overlay.iter().all(|(_, values)| !values.spilled()));
        assert_eq!(overlay[0].1.as_slice(), ["1", "8"]);
    }

    #[test]
    fn large_flat_queries_use_a_hash_index() {
        let query = [
            ("a", "1"),
            ("b", "2"),
            ("c", "3"),
            ("d", "4"),
            ("e", "5"),
            ("f", "6"),
            ("g", "7"),
            ("h", "8"),
            ("i", "9"),
        ];
        let (overlay, index) = group_flat_query(&query).expect("query groups");
        assert!(overlay.spilled());
        assert_eq!(index.expect("large query is indexed").len(), query.len());
    }

    #[test]
    fn repeated_flat_query_does_not_build_a_hash_index() {
        let query = [("tag", "a"); 16];
        let (overlay, index) = group_flat_query(&query).expect("query groups");
        assert_eq!(overlay.len(), 1);
        assert!(index.is_none());
    }

    #[test]
    fn repeated_query_keys_are_presented_as_a_sequence() {
        let decoded: Option<Result<Shelf, _>> = try_decode_overlay(
            &RequestBodyKind::None,
            &[("shelf", "first"), ("theme", "history"), ("shelf", "last")],
            b"",
        );
        let _ = decoded.expect("flat query uses overlay").expect_err("duplicate scalar");
    }

    #[test]
    fn repeated_query_values_preserve_their_order() {
        #[derive(Debug, Deserialize, PartialEq)]
        struct Tags {
            tag: Vec<String>,
        }

        let decoded: Tags = decode_flat(None, &[("tag", "first"), ("tag", "second")]).expect("decodes");
        assert_eq!(
            decoded,
            Tags {
                tag: vec!["first".to_owned(), "second".to_owned()]
            }
        );
    }

    #[test]
    fn try_decode_overlay_takes_the_fast_path_for_flat_inputs() {
        let decoded: Option<Result<Shelf, _>> = try_decode_overlay(&RequestBodyKind::None, &[("shelf", "7"), ("theme", "history")], b"");
        let shelf = decoded.expect("flat inputs take the overlay fast path").expect("decodes");
        assert_eq!(
            shelf,
            Shelf {
                shelf: "7".to_owned(),
                theme: "history".to_owned()
            }
        );
    }

    #[test]
    fn try_decode_overlay_handles_dotted_keys_and_declines_an_unoverlaid_field_body() {
        let nested: Option<Result<Shelf, _>> = try_decode_overlay(&RequestBodyKind::None, &[("shelf.id", "7")], b"");
        let _ = nested
            .expect("dotted query uses the tree overlay")
            .expect_err("unknown nested field");

        let field_mapped: Option<Result<Shelf, _>> = try_decode_overlay(&RequestBodyKind::Field("shelf"), &[], b"{}");
        assert!(field_mapped.is_none(), "body-mapped field must fall back to the value path");
    }

    #[test]
    fn a_repeated_body_key_is_rejected() {
        let body = br#"{"theme":"first","shelf":"7","theme":"second"}"#;
        let error = try_decode_overlay::<Shelf>(&RequestBodyKind::Whole, &[], body)
            .expect("flat whole-body input takes the overlay path")
            .expect_err("duplicate body fields are rejected");
        assert!(error.to_string().contains("duplicate field"));
    }

    #[test]
    fn query_does_not_rescue_a_repeated_body_key() {
        let body = br#"{"theme":"first","shelf":"body","theme":"second"}"#;
        let error = decode_flat::<Shelf>(Some(body), &[("shelf", "query")]).expect_err("duplicate body fields are rejected");
        assert!(error.to_string().contains("duplicate field"));
    }

    #[test]
    fn query_overrides_a_body_field_spelled_in_the_other_case() {
        #[derive(Debug, Deserialize, PartialEq)]
        struct Options {
            #[serde(alias = "includeArchived")]
            include_archived: bool,
        }

        let body = br#"{"includeArchived":false}"#;
        let decoded: Options = decode_flat(Some(body), &[("include_archived", "true")]).expect("query overrides camelCase body field");
        assert_eq!(decoded, Options { include_archived: true });

        let body = br#"{"include_archived":false}"#;
        let decoded: Options = decode_flat(Some(body), &[("includeArchived", "true")]).expect("query overrides snake_case body field");
        assert_eq!(decoded, Options { include_archived: true });
    }

    #[test]
    fn a_body_field_repeated_under_mixed_case_is_rejected() {
        #[derive(Debug, Deserialize, PartialEq)]
        struct Options {
            #[serde(alias = "includeArchived")]
            include_archived: bool,
        }

        let body = br#"{"include_archived":false,"includeArchived":true}"#;
        let error = decode_flat::<Options>(Some(body), &[]).expect_err("mixed-case duplicate body fields are rejected");
        assert!(error.to_string().contains("duplicate field"));
    }

    #[derive(Debug, PartialEq)]
    enum AnyScalar {
        I64(i64),
        U64(u64),
        F64,
        String(String),
    }

    impl<'de> Deserialize<'de> for AnyScalar {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            struct AnyVisitor;

            impl Visitor<'_> for AnyVisitor {
                type Value = AnyScalar;

                #[cfg_attr(coverage_nightly, coverage(off))]
                fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                    formatter.write_str("a query scalar")
                }

                fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Self::Value, E> {
                    Ok(AnyScalar::I64(v))
                }

                fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Self::Value, E> {
                    Ok(AnyScalar::U64(v))
                }

                fn visit_f64<E: serde::de::Error>(self, _value: f64) -> Result<Self::Value, E> {
                    Ok(AnyScalar::F64)
                }

                #[cfg_attr(coverage_nightly, coverage(off))]
                fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
                    Ok(AnyScalar::String(v.to_owned()))
                }

                fn visit_string<E: serde::de::Error>(self, v: String) -> Result<Self::Value, E> {
                    Ok(AnyScalar::String(v))
                }
            }

            deserializer.deserialize_any(AnyVisitor)
        }
    }

    #[test]
    fn query_any_scalar_covers_canonical_value_kinds() {
        #[derive(Debug, Deserialize)]
        struct Scalars {
            i: AnyScalar,
            u: AnyScalar,
            nan: AnyScalar,
            infinity: AnyScalar,
            negative_infinity: AnyScalar,
            decimal: AnyScalar,
            text: AnyScalar,
        }

        let decoded: Scalars = decode_flat(
            None,
            &[
                ("i", "-2"),
                ("u", "9223372036854775808"),
                ("nan", "NaN"),
                ("infinity", "Infinity"),
                ("negative_infinity", "-Infinity"),
                ("decimal", "1.5"),
                ("text", "hello"),
            ],
        )
        .expect("canonical scalars decode");
        assert_eq!(decoded.i, AnyScalar::I64(-2));
        assert_eq!(decoded.u, AnyScalar::U64(9_223_372_036_854_775_808));
        assert_eq!(decoded.nan, AnyScalar::F64);
        assert_eq!(decoded.infinity, AnyScalar::F64);
        assert_eq!(decoded.negative_infinity, AnyScalar::F64);
        assert_eq!(decoded.decimal, AnyScalar::F64);
        assert_eq!(decoded.text, AnyScalar::String("hello".to_owned()));
    }

    #[test]
    fn typed_query_scalars_cover_direct_deserializer_methods() {
        #[derive(Debug)]
        struct ViaStr(String);

        impl<'de> Deserialize<'de> for ViaStr {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                struct StrVisitor;

                impl Visitor<'_> for StrVisitor {
                    type Value = ViaStr;

                    #[cfg_attr(coverage_nightly, coverage(off))]
                    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                        formatter.write_str("a string")
                    }

                    #[cfg_attr(coverage_nightly, coverage(off))]
                    fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
                        Ok(ViaStr(v.to_owned()))
                    }

                    fn visit_string<E: serde::de::Error>(self, v: String) -> Result<Self::Value, E> {
                        Ok(ViaStr(v))
                    }
                }

                deserializer.deserialize_str(StrVisitor)
            }
        }

        #[derive(Debug, Deserialize)]
        struct Typed {
            integer: i32,
            boolean: bool,
            character: char,
            float32: f32,
            float64: f64,
            nan: f32,
            infinity: f64,
            negative_infinity: f64,
            optional: Option<i32>,
            via_str: ViaStr,
        }

        let decoded: Typed = decode_flat(
            None,
            &[
                ("integer", "7"),
                ("boolean", "false"),
                ("character", "x"),
                ("float32", "1.25"),
                ("float64", "2.5"),
                ("nan", "NaN"),
                ("infinity", "Infinity"),
                ("negative_infinity", "-Infinity"),
                ("optional", "9"),
                ("via_str", "text"),
            ],
        )
        .expect("typed scalars decode");
        assert_eq!(decoded.integer, 7);
        assert!(!decoded.boolean);
        assert_eq!(decoded.character, 'x');
        assert!((decoded.float32 - 1.25).abs() < f32::EPSILON);
        assert!((decoded.float64 - 2.5).abs() < f64::EPSILON);
        assert!(decoded.nan.is_nan());
        assert!(decoded.infinity.is_infinite() && decoded.infinity.is_sign_positive());
        assert!(decoded.negative_infinity.is_infinite() && decoded.negative_infinity.is_sign_negative());
        assert_eq!(decoded.optional, Some(9));
        assert_eq!(decoded.via_str.0, "text");

        let _ = decode_flat::<Typed>(None, &[("boolean", "not-bool")]).expect_err("invalid boolean");
        let _ = decode_flat::<Typed>(None, &[("float32", "3.5e38")]).expect_err("out-of-range float");
    }

    #[test]
    fn noncanonical_any_numbers_are_rejected() {
        #[derive(Debug, Deserialize)]
        #[expect(dead_code, reason = "the field is only exercised through failing deserialization")]
        struct One {
            value: AnyScalar,
        }

        let digit_prefixed = decode_flat::<One>(None, &[("value", "1e+")]).expect_err("malformed number");
        assert!(digit_prefixed.to_string().contains("canonical protobuf JSON"));
        let named = decode_flat::<One>(None, &[("value", "inf")]).expect_err("noncanonical infinity");
        assert!(named.to_string().contains("canonical protobuf JSON"));
    }

    #[test]
    fn tree_overlay_covers_optional_nested_values_and_conflicts() {
        #[derive(Debug, Deserialize, PartialEq)]
        struct Root {
            nested: Option<Nested>,
        }
        #[derive(Debug, Deserialize, PartialEq)]
        struct Nested {
            value: i32,
        }

        let decoded: Root = decode_tree(None, &[("nested.value", "7")]).expect("optional nested query");
        assert_eq!(
            decoded,
            Root {
                nested: Some(Nested { value: 7 })
            }
        );
        let _ = decode_tree::<Root>(Some(b"{"), &[("nested.value", "7")]).expect_err("invalid body");
        let _ = decode_tree::<Root>(None, &[("nested.value", "7"), ("nested", "8")]).expect_err("conflicting query paths");
    }

    #[test]
    fn field_overlay_rejects_an_invalid_body() {
        let decoded: Option<Result<Shelf, _>> = try_decode_overlay(&RequestBodyKind::Field("shelf"), &[("theme", "history")], b"{");
        let _ = decoded.expect("field query uses overlay").expect_err("invalid field body");
    }

    #[test]
    fn tree_map_rejects_a_value_requested_before_its_key() {
        struct StringSeed;

        impl<'de> DeserializeSeed<'de> for StringSeed {
            type Value = String;

            #[cfg_attr(coverage_nightly, coverage(off))]
            fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
                String::deserialize(deserializer)
            }
        }

        let mut map = TreeMap {
            body: Vec::new().into_iter(),
            query: Vec::new().into_iter(),
            pending: None,
        };
        let error = map.next_value_seed(StringSeed).expect_err("next_key_seed must be called first");
        assert!(error.to_string().contains("before key"));
    }
}
