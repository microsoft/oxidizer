// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! A single-pass overlay deserializer that merges captured path/query values
//! onto a JSON body without materializing the whole body as a
//! [`serde_json::Value`] tree.
//!
//! The general merge path in [`super::decode_request`] parses the body into a
//! [`serde_json::Value`], overlays the path/query fields, and then re-walks that
//! tree into `T` — two full deserialization passes plus an owned intermediate tree of
//! the entire request. For the common shape where every path variable and query
//! key targets a *top-level* field (no `a.b` nesting), that is avoidable: this
//! module scans only the body's top-level object into borrowed
//! [`RawValue`](serde_json::value::RawValue) slices (nested content stays as
//! un-parsed JSON text) and presents `T` a merged map whose overlay fields
//! shadow the body's, deserializing each field straight into `T` in one pass.
//!
//! Overlay values are presented as JSON strings, exactly as the `Value` path's
//! `Value::String` inserts do, so proto3 JSON's quoted-scalar acceptance (via
//! `pbjson`) is unchanged. Anything this path does not cover — a nested field
//! path, a dotted query key, or a body-mapped field ([`RequestBodyKind::Field`])
//! — returns [`None`] so the caller falls back to the `Value` merge path with
//! identical behaviour.

use core::fmt;
use std::borrow::Cow;

use serde::de::value::StrDeserializer;
use serde::de::{DeserializeOwned, DeserializeSeed, Deserializer, MapAccess, Visitor};
use serde_json::value::RawValue;

use super::request_body_kind::RequestBodyKind;
use super::{TranscodeError, percent};

/// Attempts the single-pass overlay decode. Returns [`None`] when the request
/// shape is not one this path handles (a dotted query key, or a body-mapped
/// field), so the caller falls back to the `Value` merge path.
///
/// Only [`RequestBodyKind::None`] and [`RequestBodyKind::Whole`] with
/// exclusively top-level (flat) query keys are handled here.
pub(crate) fn try_decode_overlay<T: DeserializeOwned>(
    body_kind: &RequestBodyKind,
    query: &[(&str, &str)],
    body: &[u8],
) -> Option<Result<T, TranscodeError>> {
    // A dotted query key needs the deep-merge the `Value` path provides; a
    // body-mapped field is left to that path too.
    if !all_flat(query) {
        return None;
    }
    let whole_body = match body_kind {
        RequestBodyKind::None => None,
        RequestBodyKind::Whole => Some(body),
        RequestBodyKind::Field(_) => return None,
    };

    Some(decode_flat(whole_body, query))
}

/// Returns `true` if every query key targets a single top-level field, so the
/// overlay's flat shadow-and-append merge is exact.
fn all_flat(query: &[(&str, &str)]) -> bool {
    query.iter().all(|(key, _)| !key.contains('.'))
}

/// Deserializes `T` from the (optional) `body` overlaid with the flat `query`,
/// in a single pass.
fn decode_flat<T: DeserializeOwned>(body: Option<&[u8]>, query: &[(&str, &str)]) -> Result<T, TranscodeError> {
    // Build the overlay from the query parameters; within the group a later entry
    // shadows an earlier one (last write wins), matching the `Value` path's
    // successive inserts.
    let mut overlay: Vec<(&str, Cow<'_, str>)> = Vec::with_capacity(query.len());
    for (key, value) in query {
        // Query values are percent-decoded (with `+` as a space); a borrowed
        // `Cow` when nothing needs decoding keeps the common case allocation-free.
        upsert(&mut overlay, key, percent::decode_query(value));
    }

    // Scan only the body's top level; nested values stay as borrowed `RawValue`
    // JSON text, parsed straight into `T` when their field is read.
    let mut body_entries = match body {
        Some(bytes) if !bytes.is_empty() => match serde_json::from_slice::<BodyTop<'_>>(bytes) {
            Ok(top) => top.entries,
            // A `BodyTop` parse fails only for malformed JSON or a non-object
            // body; reproduce the `Value` path's exact split (a body error for the
            // former, a structure error for the latter).
            Err(_) => return Err(classify_body_error(bytes)),
        },
        _ => Vec::new(),
    };

    // Drop any body field shadowed by an overlay field so the overlay wins and
    // `T` never sees the key twice. Scanning `overlay` in place avoids allocating
    // a separate owned key set just for the membership test.
    body_entries.retain(|(key, _)| !overlay.iter().any(|(overlay_key, _)| *overlay_key == key.as_str()));

    let map = OverlayMap {
        body: body_entries.into_iter(),
        overlay: overlay.into_iter(),
        pending: None,
    };
    T::deserialize(OverlayDeserializer { map }).map_err(TranscodeError::deserialize)
}

/// Inserts `value` for `key` into `overlay`, overwriting any existing entry so
/// the last write wins.
fn upsert<'p>(overlay: &mut Vec<(&'p str, Cow<'p, str>)>, key: &'p str, value: Cow<'p, str>) {
    if let Some(slot) = overlay.iter_mut().find(|(k, _)| *k == key) {
        slot.1 = value;
    } else {
        overlay.push((key, value));
    }
}

/// Classifies a body that failed to scan as a top-level object, matching the
/// `Value` merge path: malformed JSON is a body error (carrying the `serde_json`
/// source), while valid-but-not-an-object JSON is a structure error (no source).
fn classify_body_error(bytes: &[u8]) -> TranscodeError {
    match serde_json::from_slice::<serde_json::Value>(bytes) {
        Err(source) => TranscodeError::body(source),
        Ok(_) => TranscodeError::structure("request body must be a JSON object"),
    }
}

/// The body's top-level object captured as `(name, raw-value)` pairs, with the
/// nested value left as borrowed JSON text. Duplicate names keep the last (the
/// `serde_json::Map` semantics the `Value` path relies on).
struct BodyTop<'de> {
    entries: Vec<(String, &'de RawValue)>,
}

impl<'de> serde::Deserialize<'de> for BodyTop<'de> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct TopVisitor;

        impl<'de> Visitor<'de> for TopVisitor {
            type Value = BodyTop<'de>;

            // Human-readable diagnostic text with no API contract (like `Debug`),
            // so its return value is not worth mutation-testing.
            #[cfg_attr(test, mutants::skip)]
            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a JSON object")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut entries: Vec<(String, &'de RawValue)> = Vec::new();
                while let Some(key) = map.next_key::<String>()? {
                    let value: &'de RawValue = map.next_value()?;
                    if let Some(slot) = entries.iter_mut().find(|(k, _)| *k == key) {
                        slot.1 = value;
                    } else {
                        entries.push((key, value));
                    }
                }
                Ok(BodyTop { entries })
            }
        }

        deserializer.deserialize_map(TopVisitor)
    }
}

/// The value staged by [`OverlayMap::next_key_seed`] for the following
/// [`OverlayMap::next_value_seed`] call.
enum Pending<'de> {
    /// A body field's borrowed JSON text, deserialized directly into `T`.
    Raw(&'de RawValue),
    /// An overlay field, presented to `T` as a JSON string.
    Str(Cow<'de, str>),
}

/// A [`MapAccess`] yielding the (non-shadowed) body fields first, then the
/// overlay fields, deserializing each straight into `T`.
struct OverlayMap<'de, B, O>
where
    B: Iterator<Item = (String, &'de RawValue)>,
    O: Iterator<Item = (&'de str, Cow<'de, str>)>,
{
    body: B,
    overlay: O,
    pending: Option<Pending<'de>>,
}

impl<'de, B, O> MapAccess<'de> for OverlayMap<'de, B, O>
where
    B: Iterator<Item = (String, &'de RawValue)>,
    O: Iterator<Item = (&'de str, Cow<'de, str>)>,
{
    type Error = serde_json::Error;

    fn next_key_seed<K: DeserializeSeed<'de>>(&mut self, seed: K) -> Result<Option<K::Value>, Self::Error> {
        if let Some((key, value)) = self.body.next() {
            self.pending = Some(Pending::Raw(value));
            return seed.deserialize(StrDeserializer::<serde_json::Error>::new(&key)).map(Some);
        }
        if let Some((key, value)) = self.overlay.next() {
            self.pending = Some(Pending::Str(value));
            return seed.deserialize(StrDeserializer::<serde_json::Error>::new(key)).map(Some);
        }
        Ok(None)
    }

    // The `None` arm guards a serde protocol violation (a value requested before
    // a key) that a correct `Deserializer` never triggers; it is defensive only.
    #[cfg_attr(coverage_nightly, coverage(off))]
    fn next_value_seed<V: DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value, Self::Error> {
        match self.pending.take() {
            Some(Pending::Raw(raw)) => {
                let mut deserializer = serde_json::Deserializer::from_str(raw.get());
                seed.deserialize(&mut deserializer)
            }
            // Overlay values are strings, exactly as the `Value` path's
            // `Value::String` inserts, so scalar coercion via `pbjson` matches.
            Some(Pending::Str(value)) => seed.deserialize(StrDeserializer::<serde_json::Error>::new(value.as_ref())),
            None => Err(serde::de::Error::custom("value requested before key")),
        }
    }
}

/// A [`Deserializer`] presenting the merged [`OverlayMap`] as a map to `T`.
struct OverlayDeserializer<'de, B, O>
where
    B: Iterator<Item = (String, &'de RawValue)>,
    O: Iterator<Item = (&'de str, Cow<'de, str>)>,
{
    map: OverlayMap<'de, B, O>,
}

impl<'de, B, O> Deserializer<'de> for OverlayDeserializer<'de, B, O>
where
    B: Iterator<Item = (String, &'de RawValue)>,
    O: Iterator<Item = (&'de str, Cow<'de, str>)>,
{
    type Error = serde_json::Error;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        visitor.visit_map(self.map)
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
        bytes byte_buf option unit unit_struct newtype_struct seq tuple
        tuple_struct map struct enum identifier ignored_any
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
    fn a_repeated_query_key_keeps_the_last_occurrence() {
        // Two query entries for the same key: the overlay's `upsert` overwrites
        // the earlier value so the last write wins (matching the `Value` path's
        // successive inserts).
        let decoded: Shelf = try_decode_overlay(
            &RequestBodyKind::None,
            &[("shelf", "first"), ("theme", "history"), ("shelf", "last")],
            b"",
        )
        .expect("flat query takes the overlay path")
        .expect("decodes");
        assert_eq!(
            decoded,
            Shelf {
                shelf: "last".to_owned(),
                theme: "history".to_owned()
            }
        );
    }

    #[test]
    fn try_decode_overlay_takes_the_fast_path_for_flat_inputs() {
        // Flat query with no body: the overlay path applies and decodes
        // successfully (a `Some(Ok(_))`, never `None`).
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
    fn try_decode_overlay_declines_dotted_and_body_mapped_shapes() {
        // A dotted query key is not flat, so the overlay path declines (`None`).
        let nested: Option<Result<Shelf, _>> = try_decode_overlay(&RequestBodyKind::None, &[("shelf.id", "7")], b"");
        assert!(nested.is_none(), "dotted query key must fall back to the value path");

        // A body-mapped field is left to the value path too.
        let field_mapped: Option<Result<Shelf, _>> = try_decode_overlay(&RequestBodyKind::Field("shelf"), &[], b"{}");
        assert!(field_mapped.is_none(), "body-mapped field must fall back to the value path");
    }

    #[test]
    fn a_repeated_body_key_keeps_the_last_occurrence() {
        // The top-level body scan de-duplicates repeated keys, last-write-wins.
        let body = br#"{"theme":"first","shelf":"7","theme":"second"}"#;
        let decoded: Shelf = try_decode_overlay(&RequestBodyKind::Whole, &[], body)
            .expect("flat whole-body input takes the overlay path")
            .expect("decodes");
        assert_eq!(
            decoded,
            Shelf {
                shelf: "7".to_owned(),
                theme: "second".to_owned()
            }
        );
    }
}
