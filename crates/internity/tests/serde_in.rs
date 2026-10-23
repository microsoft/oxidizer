// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration tests for interner-aware (`de`) deserialization.

#![cfg(feature = "serde")]

use std::collections::{BTreeMap, BTreeSet};

#[cfg(feature = "std")]
use internity::ThreadedLexicon;
use internity::de::{DeserializeIn, DeserializeSeed};
use internity::{Lexicon, LocalLexicon, Reader, Sym};
use serde::Deserialize;
use serde::de::value::StringDeserializer;
use serde::de::{Error as _, Unexpected, Visitor};

#[derive(Deserialize)]
struct Plain {
    label: String,
    weight: u32,
}

#[derive(DeserializeIn)]
struct Record {
    name: Sym,
    aliases: Vec<Sym>,
    parent: Option<Sym>,
    count: u64,
    #[internity(via_serde)]
    plain: Plain,
}

#[derive(DeserializeIn)]
struct Newtype(Sym);

#[derive(DeserializeIn)]
struct Pair(Sym, u32);

#[derive(DeserializeIn)]
struct Unit;

#[derive(DeserializeIn)]
struct Containers {
    optional: Option<Sym>,
    boxed: Box<Sym>,
    map: BTreeMap<String, Sym>,
    set: BTreeSet<String>,
    tuple: (Sym, u32),
}

fn json(input: &str) -> serde_json::Deserializer<serde_json::de::StrRead<'_>> {
    serde_json::Deserializer::from_str(input)
}

struct VisitNone;

impl<'de> serde::Deserializer<'de> for VisitNone {
    type Error = serde::de::value::Error;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        visitor.visit_none()
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
        bytes byte_buf option unit unit_struct newtype_struct seq tuple tuple_struct
        map struct enum identifier ignored_any
    }
}

struct VisitUnit;

impl<'de> serde::Deserializer<'de> for VisitUnit {
    type Error = serde::de::value::Error;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        visitor.visit_unit()
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
        bytes byte_buf option unit unit_struct newtype_struct seq tuple tuple_struct
        map struct enum identifier ignored_any
    }
}

struct Reject;

impl<'de> serde::Deserializer<'de> for Reject {
    type Error = serde::de::value::Error;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        Err(Self::Error::invalid_type(Unexpected::Bool(true), &visitor))
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
        bytes byte_buf option unit unit_struct newtype_struct seq tuple tuple_struct
        map struct enum identifier ignored_any
    }
}

#[test]
fn named_struct_map_form_into_lexicon() {
    let mut lexicon = LocalLexicon::new();
    let record: Record = lexicon
        .deserialize_in(&mut json(
            r#"{
                "name": "root",
                "aliases": ["r", "root", "r"],
                "parent": "ancestor",
                "count": 7,
                "plain": {"label": "x", "weight": 2}
            }"#,
        ))
        .unwrap();

    assert_eq!(lexicon.resolve(record.name), "root");
    assert_eq!(record.count, 7);
    assert_eq!(record.plain.label, "x");
    assert_eq!(record.plain.weight, 2);

    let aliases: Vec<&str> = record.aliases.iter().map(|s| lexicon.resolve(*s)).collect();
    assert_eq!(aliases, ["r", "root", "r"]);

    // Interned handles are deduplicated within the chosen lexicon.
    assert_eq!(record.aliases[0], record.aliases[2]);
    assert_eq!(record.aliases[1], record.name);

    assert_eq!(record.parent.map(|s| lexicon.resolve(s)), Some("ancestor"));
}

#[test]
fn deserializes_through_an_erased_lexicon() {
    // `DeserializeIn` must be usable through `&mut dyn Lexicon` (the crate
    // advertises `Box<dyn Lexicon>`), which requires the `I: ?Sized` bound.
    let mut lexicon = LocalLexicon::new();
    let record = {
        let erased: &mut dyn Lexicon = &mut lexicon;
        Record::deserialize_in(
            erased,
            &mut json(r#"["root", ["a", "root"], null, 2, {"label": "y", "weight": 0}]"#),
        )
        .unwrap()
    };

    assert_eq!(lexicon.resolve(record.name), "root");
    assert_eq!(record.count, 2);
    // Deduplication still works through the erased interner.
    assert_eq!(record.aliases[1], record.name);
}

#[test]
fn named_struct_seq_form() {
    let mut lexicon = LocalLexicon::new();
    let record: Record = lexicon
        .deserialize_in(&mut json(r#"["root", ["a"], null, 1, {"label": "y", "weight": 0}]"#))
        .unwrap();

    assert_eq!(lexicon.resolve(record.name), "root");
    assert_eq!(record.parent, None);
    assert_eq!(record.count, 1);
    assert_eq!(record.plain.label, "y");
}

#[test]
fn missing_field_is_an_error() {
    let mut lexicon = LocalLexicon::new();
    let result: Result<Record, _> = lexicon.deserialize_in(&mut json(r#"{"name": "x"}"#));
    assert!(result.is_err());
    assert!(lexicon.get("x").is_some(), "deserialization is explicitly non-transactional");
}

#[test]
fn trailing_sequence_fields_are_rejected() {
    let mut lexicon = LocalLexicon::new();
    let named: Result<Record, _> = lexicon.deserialize_in(&mut json(r#"["root", [], null, 1, {"label": "y", "weight": 0}, "extra"]"#));
    assert!(named.is_err());

    let tuple: Result<Pair, _> = lexicon.deserialize_in(&mut json(r#"["key", 1, "extra"]"#));
    assert!(tuple.is_err());
}

#[test]
fn tuple_and_unit_structs() {
    let mut lexicon = LocalLexicon::new();

    let newtype: Newtype = lexicon.deserialize_in(&mut json(r#""solo""#)).unwrap();
    assert_eq!(lexicon.resolve(newtype.0), "solo");

    let pair: Pair = lexicon.deserialize_in(&mut json(r#"["k", 9]"#)).unwrap();
    assert_eq!(lexicon.resolve(pair.0), "k");
    assert_eq!(pair.1, 9);

    let _unit: Unit = lexicon.deserialize_in(&mut json("null")).unwrap();
}

#[cfg(feature = "std")]
#[test]
fn threaded_lexicon_entry_point() {
    let lexicon = ThreadedLexicon::new();
    let record: Record = lexicon
        .deserialize_in(&mut json(
            r#"{"name": "n", "aliases": [], "parent": null, "count": 0, "plain": {"label": "z", "weight": 1}}"#,
        ))
        .unwrap();

    let reader = lexicon.freeze();
    assert_eq!(reader.resolve(record.name), "n");
}

#[test]
fn seed_reused_shares_lexicon() {
    // A single lexicon accumulates handles across multiple deserializations.
    let mut lexicon = LocalLexicon::new();
    let first: Newtype = lexicon.deserialize_in(&mut json(r#""shared""#)).unwrap();
    let second: Newtype = lexicon.deserialize_in(&mut json(r#""shared""#)).unwrap();
    assert_eq!(first.0, second.0);
}

#[test]
fn interner_trait_intern_directly() {
    let mut lexicon = LocalLexicon::new();
    let a = Lexicon::intern(&mut lexicon, "dup");
    let b = Lexicon::intern(&mut lexicon, "dup");
    assert_eq!(a, b);
}

#[cfg(feature = "std")]
#[test]
fn interner_trait_intern_on_threaded() {
    let threaded = ThreadedLexicon::new();
    let mut handle = threaded.clone();
    let sym = Lexicon::intern(&mut handle, "referenced");
    assert_eq!(threaded.freeze().resolve(sym), "referenced");
}

#[test]
fn ordinary_serde_deserialize_remains_callable() {
    let lexicon = LocalLexicon::deserialize(&mut json(r#"["a"]"#)).unwrap();
    assert_eq!(lexicon.resolve(Sym::from_u32(1).unwrap()), "a");
}

#[test]
fn common_container_implementations() {
    let mut lexicon = LocalLexicon::new();
    let value: Containers = lexicon
        .deserialize_in(&mut json(
            r#"{
                "optional": "optional",
                "boxed": "boxed",
                "map": {"key": "value"},
                "set": ["set", "set"],
                "tuple": ["tuple", 42]
            }"#,
        ))
        .unwrap();

    let reader = lexicon.freeze();
    assert_eq!(reader.resolve(value.optional.unwrap()), "optional");
    assert_eq!(reader.resolve(*value.boxed), "boxed");
    assert_eq!(value.map.len(), 1);
    let (key, mapped) = value.map.first_key_value().unwrap();
    assert_eq!(key, "key");
    assert_eq!(reader.resolve(*mapped), "value");
    assert_eq!(value.set.len(), 1);
    assert_eq!(value.set.first().unwrap(), "set");
    assert_eq!(reader.resolve(value.tuple.0), "tuple");
    assert_eq!(value.tuple.1, 42);
}

#[test]
fn option_accepts_none_and_unit() {
    let mut lexicon = LocalLexicon::new();
    assert_eq!(Option::<Sym>::deserialize_in(&mut lexicon, VisitNone).unwrap(), None);
    assert_eq!(Option::<Sym>::deserialize_in(&mut lexicon, VisitUnit).unwrap(), None);
}

#[test]
fn visitors_describe_expected_input() {
    let mut lexicon = LocalLexicon::new();
    let cases = [
        Sym::deserialize_in(&mut lexicon, Reject).unwrap_err().to_string(),
        Option::<Sym>::deserialize_in(&mut lexicon, Reject).unwrap_err().to_string(),
        Vec::<Sym>::deserialize_in(&mut lexicon, Reject).unwrap_err().to_string(),
        BTreeMap::<String, Sym>::deserialize_in(&mut lexicon, Reject)
            .unwrap_err()
            .to_string(),
        BTreeSet::<String>::deserialize_in(&mut lexicon, Reject).unwrap_err().to_string(),
        <(u32, u32, u32, u32, u32, u32, u32, u32)>::deserialize_in(&mut lexicon, Reject)
            .unwrap_err()
            .to_string(),
    ];

    for (actual, expected) in cases.iter().zip([
        "a string to intern",
        "an optional value",
        "a sequence",
        "a map",
        "a sequence",
        "a tuple of size 8",
    ]) {
        assert!(actual.contains(expected), "{actual:?} does not contain {expected:?}");
    }
}

#[cfg(feature = "std")]
#[test]
fn hash_visitors_describe_expected_input() {
    use std::collections::{HashMap, HashSet};

    let mut lexicon = LocalLexicon::new();
    let map_err = HashMap::<String, Sym>::deserialize_in(&mut lexicon, Reject)
        .unwrap_err()
        .to_string();
    let set_err = HashSet::<String>::deserialize_in(&mut lexicon, Reject).unwrap_err().to_string();

    assert!(map_err.contains("a map"), "{map_err:?}");
    assert!(set_err.contains("a sequence"), "{set_err:?}");
}

#[test]
fn owned_strings_and_plain_serde_seeds() {
    let mut lexicon = LocalLexicon::new();
    let sym = Sym::deserialize_in(&mut lexicon, StringDeserializer::<serde::de::value::Error>::new("owned".to_owned())).unwrap();
    assert_eq!(lexicon.freeze().resolve(sym), "owned");

    let value = serde::de::DeserializeSeed::deserialize(DeserializeSeed::<u32>::default(), &mut json("42")).unwrap();
    assert_eq!(value, 42);
}

/// A permissive `SeqAccess` that hands the visitor each supplied `serde_json::Value`
/// in turn and reports `None` once they are exhausted. Crucially, it performs **no**
/// independent exhaustion check: if the visitor stops early, leftover elements are
/// silently ignored. This isolates the generated visitor's own trailing-element
/// probe as the only thing that can reject an over-long sequence — unlike
/// `serde_json`, whose array deserializer independently rejects unread elements.
struct PermissiveSeq {
    values: std::vec::IntoIter<serde_json::Value>,
}

impl<'de> serde::de::SeqAccess<'de> for PermissiveSeq {
    type Error = serde::de::value::Error;

    fn next_element_seed<T>(&mut self, seed: T) -> Result<Option<T::Value>, Self::Error>
    where
        T: serde::de::DeserializeSeed<'de>,
    {
        match self.values.next() {
            Some(value) => seed.deserialize(value).map(Some).map_err(serde::de::Error::custom),
            None => Ok(None),
        }
    }
}

/// A `Deserializer` that drives struct and tuple-struct visitors through
/// [`PermissiveSeq`]. Every other entry point is unsupported.
struct PermissiveDeserializer {
    values: Vec<serde_json::Value>,
}

impl<'de> serde::Deserializer<'de> for PermissiveDeserializer {
    type Error = serde::de::value::Error;

    fn deserialize_any<V: Visitor<'de>>(self, _visitor: V) -> Result<V::Value, Self::Error> {
        Err(serde::de::Error::custom(
            "permissive deserializer only drives struct/tuple-struct sequences",
        ))
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        visitor.visit_seq(PermissiveSeq {
            values: self.values.into_iter(),
        })
    }

    fn deserialize_tuple_struct<V: Visitor<'de>>(self, _name: &'static str, _len: usize, visitor: V) -> Result<V::Value, Self::Error> {
        visitor.visit_seq(PermissiveSeq {
            values: self.values.into_iter(),
        })
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
        bytes byte_buf option unit unit_struct newtype_struct seq tuple map enum
        identifier ignored_any
    }
}

#[test]
fn trailing_elements_rejected_by_generated_guard_not_the_format() {
    use serde_json::Value;

    // Named struct: five fields, six values supplied. serde_json is bypassed, so
    // only the generated visit_seq's post-read probe can reject the extra element.
    let mut lexicon = LocalLexicon::new();
    let named: Result<Record, _> = Record::deserialize_in(
        &mut lexicon,
        PermissiveDeserializer {
            values: vec![
                Value::String("root".to_owned()),
                Value::Array(vec![]),
                Value::Null,
                Value::from(1u64),
                serde_json::json!({"label": "y", "weight": 0}),
                Value::String("extra".to_owned()),
            ],
        },
    );
    assert!(named.is_err(), "generated named-struct guard must reject the trailing element");

    // Tuple struct: two fields, three values supplied.
    let mut lexicon = LocalLexicon::new();
    let tuple: Result<Pair, _> = Pair::deserialize_in(
        &mut lexicon,
        PermissiveDeserializer {
            values: vec![
                Value::String("key".to_owned()),
                Value::from(1u64),
                Value::String("extra".to_owned()),
            ],
        },
    );
    assert!(tuple.is_err(), "generated tuple-struct guard must reject the trailing element");

    // Sanity: the exact-arity inputs still deserialize through the same permissive path.
    let mut lexicon = LocalLexicon::new();
    let ok: Pair = Pair::deserialize_in(
        &mut lexicon,
        PermissiveDeserializer {
            values: vec![Value::String("key".to_owned()), Value::from(1u64)],
        },
    )
    .expect("exact-arity tuple struct deserializes");
    assert_eq!(lexicon.resolve(ok.0), "key");
    assert_eq!(ok.1, 1);
}
