// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration tests for reader-aware (`se`) serialization, including
//! round-trips with the sibling `DeserializeIn` derive.

#![cfg(feature = "serde")]

use std::collections::{BTreeMap, BTreeSet};
use std::marker::PhantomData;

use internity::de::DeserializeIn;
use internity::se::{SerializeIn, SerializeInWith, SerializeReader};
use internity::{LocalLexicon, Reader, Sym};
use serde::{Deserialize, Serialize, Serializer};

#[derive(Serialize, Deserialize, PartialEq, Debug)]
struct Plain {
    label: String,
    weight: u32,
}

#[derive(SerializeIn, DeserializeIn)]
struct Record {
    name: Sym,
    aliases: Vec<Sym>,
    parent: Option<Sym>,
    count: u64,
    #[internity(via_serde)]
    plain: Plain,
}

#[derive(SerializeIn, DeserializeIn)]
struct Newtype(Sym);

#[derive(SerializeIn, DeserializeIn)]
struct Pair(Sym, u32);

#[derive(SerializeIn, DeserializeIn)]
struct Unit;

#[derive(SerializeIn, DeserializeIn)]
struct Containers {
    optional: Option<Sym>,
    boxed: Box<Sym>,
    map: BTreeMap<String, Sym>,
    set: BTreeSet<String>,
    tuple: (Sym, u32),
}

#[derive(SerializeIn, DeserializeIn)]
#[expect(clippy::type_complexity, reason = "exercising wide context-aware tuple arities")]
struct WideTuples {
    nine: (Sym, Sym, Sym, Sym, Sym, Sym, Sym, Sym, Sym),
    sixteen: (Sym, Sym, Sym, Sym, Sym, Sym, Sym, Sym, Sym, Sym, Sym, Sym, Sym, Sym, Sym, Sym),
}

#[derive(SerializeIn, DeserializeIn)]
#[serde(rename_all = "camelCase")]
struct Renamed {
    first_name: Sym,
    #[serde(rename = "years")]
    age: u32,
}

#[derive(SerializeIn, DeserializeIn)]
#[serde(transparent)]
struct TransparentNamed {
    inner: Sym,
}

#[derive(SerializeIn, DeserializeIn)]
#[serde(transparent)]
struct TransparentNewtype(Sym);

#[derive(SerializeIn, DeserializeIn)]
#[serde(transparent)]
struct TransparentWithPhantom {
    inner: Sym,
    marker: PhantomData<u8>,
}

#[derive(SerializeIn, DeserializeIn)]
struct Skipping {
    name: Sym,
    #[serde(skip)]
    ignored: u32,
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde `serialize_with` requires the `fn(&T, S)` signature"
)]
fn bump<S: Serializer>(value: &u32, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_u32(*value + 1)
}

#[derive(SerializeIn)]
struct WithSerializeWith {
    name: Sym,
    #[serde(serialize_with = "bump")]
    bumped: u32,
}

/// Serialize `value` (resolving handles against `reader`) to a JSON string.
fn to_json<T: SerializeIn<R>, R: Reader>(value: &T, reader: &R) -> String {
    serde_json::to_string(&SerializeInWith::new(value, reader)).expect("serialization succeeds")
}

/// Deserialize a fresh value from `json` into a new lexicon, returning both.
fn from_json<T>(json: &str) -> (LocalLexicon, T)
where
    T: for<'de> DeserializeIn<'de, LocalLexicon>,
{
    let mut lexicon = LocalLexicon::new();
    let value = lexicon
        .deserialize_in(&mut serde_json::Deserializer::from_str(json))
        .expect("deserialization succeeds");
    (lexicon, value)
}

#[test]
fn named_struct_round_trips_via_string() {
    let mut lexicon = LocalLexicon::new();
    let record = Record {
        name: lexicon.intern("root"),
        aliases: vec![lexicon.intern("r"), lexicon.intern("root")],
        parent: Some(lexicon.intern("ancestor")),
        count: 7,
        plain: Plain {
            label: "x".to_owned(),
            weight: 2,
        },
    };
    let reader = lexicon.freeze();
    let json = to_json(&record, &reader);
    assert_eq!(
        json,
        r#"{"name":"root","aliases":["r","root"],"parent":"ancestor","count":7,"plain":{"label":"x","weight":2}}"#
    );

    let (restored, back): (LocalLexicon, Record) = from_json(&json);
    assert_eq!(restored.resolve(back.name), "root");
    let aliases: Vec<&str> = back.aliases.iter().map(|s| restored.resolve(*s)).collect();
    assert_eq!(aliases, ["r", "root"]);
    assert_eq!(back.parent.map(|s| restored.resolve(s)), Some("ancestor"));
    assert_eq!(back.count, 7);
    assert_eq!(back.plain, record.plain);
}

#[test]
fn tuple_newtype_and_unit_round_trip() {
    let mut lexicon = LocalLexicon::new();
    let newtype = Newtype(lexicon.intern("solo"));
    let pair = Pair(lexicon.intern("key"), 9);
    let reader = lexicon.freeze();

    assert_eq!(to_json(&newtype, &reader), r#""solo""#);
    assert_eq!(to_json(&pair, &reader), r#"["key",9]"#);
    assert_eq!(to_json(&Unit, &reader), "null");

    let (l1, back): (LocalLexicon, Newtype) = from_json(r#""solo""#);
    assert_eq!(l1.resolve(back.0), "solo");
    let (l2, back): (LocalLexicon, Pair) = from_json(r#"["key",9]"#);
    assert_eq!(l2.resolve(back.0), "key");
    assert_eq!(back.1, 9);
    let (_l3, _unit): (LocalLexicon, Unit) = from_json("null");
}

#[test]
fn wide_tuples_round_trip_through_arity_sixteen() {
    let mut lexicon = LocalLexicon::new();
    let s = |lex: &mut LocalLexicon, i: u32| lex.intern(format!("t{i}"));
    let nine = (
        s(&mut lexicon, 0),
        s(&mut lexicon, 1),
        s(&mut lexicon, 2),
        s(&mut lexicon, 3),
        s(&mut lexicon, 4),
        s(&mut lexicon, 5),
        s(&mut lexicon, 6),
        s(&mut lexicon, 7),
        s(&mut lexicon, 8),
    );
    let sixteen = (
        s(&mut lexicon, 100),
        s(&mut lexicon, 101),
        s(&mut lexicon, 102),
        s(&mut lexicon, 103),
        s(&mut lexicon, 104),
        s(&mut lexicon, 105),
        s(&mut lexicon, 106),
        s(&mut lexicon, 107),
        s(&mut lexicon, 108),
        s(&mut lexicon, 109),
        s(&mut lexicon, 110),
        s(&mut lexicon, 111),
        s(&mut lexicon, 112),
        s(&mut lexicon, 113),
        s(&mut lexicon, 114),
        s(&mut lexicon, 115),
    );
    let value = WideTuples { nine, sixteen };
    let reader = lexicon.freeze();
    let json = to_json(&value, &reader);

    let (restored, back): (LocalLexicon, WideTuples) = from_json(&json);
    assert_eq!(restored.resolve(back.nine.0), "t0");
    assert_eq!(restored.resolve(back.nine.8), "t8");
    assert_eq!(restored.resolve(back.sixteen.0), "t100");
    assert_eq!(restored.resolve(back.sixteen.15), "t115");
}

#[test]
fn common_containers_round_trip() {
    let mut lexicon = LocalLexicon::new();
    let value = Containers {
        optional: Some(lexicon.intern("optional")),
        boxed: Box::new(lexicon.intern("boxed")),
        map: BTreeMap::from([("key".to_owned(), lexicon.intern("value"))]),
        set: BTreeSet::from(["set".to_owned()]),
        tuple: (lexicon.intern("tuple"), 42),
    };
    let reader = lexicon.freeze();
    let json = to_json(&value, &reader);

    let (restored, back): (LocalLexicon, Containers) = from_json(&json);
    assert_eq!(restored.resolve(back.optional.unwrap()), "optional");
    assert_eq!(restored.resolve(*back.boxed), "boxed");
    assert_eq!(restored.resolve(*back.map.get("key").unwrap()), "value");
    assert!(back.set.contains("set"));
    assert_eq!(restored.resolve(back.tuple.0), "tuple");
    assert_eq!(back.tuple.1, 42);
}

#[test]
fn option_none_serializes_as_null() {
    let mut lexicon = LocalLexicon::new();
    let value = Containers {
        optional: None,
        boxed: Box::new(lexicon.intern("b")),
        map: BTreeMap::new(),
        set: BTreeSet::new(),
        tuple: (lexicon.intern("t"), 1),
    };
    let reader = lexicon.freeze();
    let json = to_json(&value, &reader);
    assert!(json.contains(r#""optional":null"#));
}

#[test]
fn rename_all_and_field_rename_round_trip() {
    let mut lexicon = LocalLexicon::new();
    let value = Renamed {
        first_name: lexicon.intern("ada"),
        age: 36,
    };
    let reader = lexicon.freeze();
    let json = to_json(&value, &reader);
    assert_eq!(json, r#"{"firstName":"ada","years":36}"#);

    let (restored, back): (LocalLexicon, Renamed) = from_json(&json);
    assert_eq!(restored.resolve(back.first_name), "ada");
    assert_eq!(back.age, 36);
}

#[test]
fn transparent_structs_round_trip() {
    let mut lexicon = LocalLexicon::new();
    let named = TransparentNamed {
        inner: lexicon.intern("n"),
    };
    let newtype = TransparentNewtype(lexicon.intern("w"));
    let phantom = TransparentWithPhantom {
        inner: lexicon.intern("p"),
        marker: PhantomData,
    };
    let reader = lexicon.freeze();

    assert_eq!(to_json(&named, &reader), r#""n""#);
    assert_eq!(to_json(&newtype, &reader), r#""w""#);
    assert_eq!(to_json(&phantom, &reader), r#""p""#);

    let (l1, back): (LocalLexicon, TransparentNamed) = from_json(r#""n""#);
    assert_eq!(l1.resolve(back.inner), "n");
    let (l2, back): (LocalLexicon, TransparentNewtype) = from_json(r#""w""#);
    assert_eq!(l2.resolve(back.0), "w");
    let (l3, back): (LocalLexicon, TransparentWithPhantom) = from_json(r#""p""#);
    assert_eq!(l3.resolve(back.inner), "p");
}

#[test]
fn skipped_field_is_omitted_and_defaulted() {
    let mut lexicon = LocalLexicon::new();
    let value = Skipping {
        name: lexicon.intern("k"),
        ignored: 99,
    };
    let reader = lexicon.freeze();
    let json = to_json(&value, &reader);
    assert_eq!(json, r#"{"name":"k"}"#);

    let (restored, back): (LocalLexicon, Skipping) = from_json(&json);
    assert_eq!(restored.resolve(back.name), "k");
    assert_eq!(back.ignored, 0);
}

#[test]
fn serialize_with_adapter_is_applied() {
    let mut lexicon = LocalLexicon::new();
    let value = WithSerializeWith {
        name: lexicon.intern("k"),
        bumped: 41,
    };
    let reader = lexicon.freeze();
    let json = to_json(&value, &reader);
    assert_eq!(json, r#"{"name":"k","bumped":42}"#);
}

// Ensure a generated adapter name cannot shadow the user's `serialize_with`
// function.
#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde `serialize_with` requires the `fn(&T, S)` signature"
)]
#[expect(non_snake_case, reason = "deliberately collides with a generated adapter name")]
fn __InternitySerializeWith1ForCollideSer<S: Serializer>(value: &u32, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_u32(*value + 1)
}

#[derive(SerializeIn)]
struct CollideSer {
    name: Sym,
    #[serde(serialize_with = "__InternitySerializeWith1ForCollideSer")]
    bumped: u32,
}

#[test]
fn serialize_with_colliding_helper_name_resolves_to_user_function() {
    let mut lexicon = LocalLexicon::new();
    let value = CollideSer {
        name: lexicon.intern("k"),
        bumped: 41,
    };
    let reader = lexicon.freeze();
    let json = to_json(&value, &reader);
    assert_eq!(json, r#"{"name":"k","bumped":42}"#);
}

#[test]
fn large_tuple_serializes_each_element() {
    let mut lexicon = LocalLexicon::new();
    let a = lexicon.intern("a");
    let reader = lexicon.freeze();
    let tuple = (a, 1u32, 2u32, 3u32, 4u32, 5u32, 6u32, 7u32);
    let json = to_json(&tuple, &reader);
    assert_eq!(json, r#"["a",1,2,3,4,5,6,7]"#);
}

#[test]
fn scalar_and_string_serialize_in_directly() {
    let lexicon = LocalLexicon::new();
    let reader = lexicon.freeze();
    assert_eq!(to_json(&7u64, &reader), "7");
    assert_eq!(to_json(&"literal".to_owned(), &reader), r#""literal""#);
}

#[test]
fn serialize_reader_emits_corpus() {
    let mut lexicon = LocalLexicon::new();
    lexicon.intern("a");
    lexicon.intern("b");
    let reader = lexicon.freeze();
    let json = serde_json::to_string(&SerializeReader(&reader)).unwrap();
    assert_eq!(json, r#"["a","b"]"#);
}

#[test]
fn out_of_range_handle_is_a_serialize_error() {
    // An out-of-range handle (here, one interned elsewhere but past the target
    // reader's length) is unresolvable, so serialization fails. This is a range
    // check, not provenance validation.
    let mut other = LocalLexicon::new();
    let foreign = other.intern("zzz");

    let empty = LocalLexicon::new();
    let reader = empty.freeze();
    let error = serde_json::to_string(&SerializeInWith::new(&foreign, &reader)).unwrap_err();
    assert!(error.to_string().contains("out of range"), "{error}");
}

#[test]
fn in_range_foreign_handle_serializes_the_wrong_string() {
    // `SerializeIn` cannot validate provenance: an in-range foreign handle
    // resolves to the string occupying that slot in the target reader.
    let mut source = LocalLexicon::new();
    let handle = source.intern("from-source");

    let mut target = LocalLexicon::new();
    let _ = target.intern("from-target"); // same first dense slot as `handle`
    let reader = target.freeze();

    let json = serde_json::to_string(&SerializeInWith::new(&handle, &reader)).unwrap();
    assert_eq!(json, r#""from-target""#);
}

#[test]
fn serializes_through_an_erased_reader() {
    // The serialize side is `R: Reader + ?Sized`, so a `dyn Reader` works too.
    let mut lexicon = LocalLexicon::new();
    let pair = Pair(lexicon.intern("key"), 9);
    let reader = lexicon.freeze();
    let erased: &dyn Reader = &reader;

    let json = serde_json::to_string(&SerializeInWith::new(&pair, erased)).expect("serializes via dyn Reader");
    assert_eq!(json, r#"["key",9]"#);

    // `SerializeReader` also accepts an erased reader.
    let corpus = serde_json::to_string(&SerializeReader(erased)).expect("serializes corpus via dyn Reader");
    assert_eq!(corpus, r#"["key"]"#);
}

#[cfg(feature = "std")]
#[test]
fn hash_collections_round_trip() {
    use std::collections::{HashMap, HashSet};

    let mut lexicon = LocalLexicon::new();
    let mut map: HashMap<String, Sym> = HashMap::new();
    let _ = map.insert("a".to_owned(), lexicon.intern("alpha"));
    let _ = map.insert("b".to_owned(), lexicon.intern("beta"));
    let mut set: HashSet<String> = HashSet::new();
    let _ = set.insert("x".to_owned());
    let _ = set.insert("y".to_owned());
    let reader = lexicon.freeze();

    let map_json = to_json(&map, &reader);
    let set_json = to_json(&set, &reader);

    let (rmap_lex, back_map): (LocalLexicon, HashMap<String, Sym>) = from_json(&map_json);
    assert_eq!(back_map.len(), 2);
    assert_eq!(rmap_lex.resolve(back_map["a"]), "alpha");
    assert_eq!(rmap_lex.resolve(back_map["b"]), "beta");

    let (_rset_lex, back_set): (LocalLexicon, HashSet<String>) = from_json(&set_json);
    assert_eq!(back_set.len(), 2);
    assert!(back_set.contains("x"));
    assert!(back_set.contains("y"));
}

#[cfg(feature = "std")]
#[test]
fn sym_keyed_collections_round_trip() {
    use internity::{SymMap, SymSet};

    let mut lexicon = LocalLexicon::new();
    let mut map: SymMap<u32> = SymMap::default();
    let _ = map.insert(lexicon.intern("one"), 1);
    let _ = map.insert(lexicon.intern("two"), 2);
    let mut set: SymSet = SymSet::default();
    let _ = set.insert(lexicon.intern("red"));
    let _ = set.insert(lexicon.intern("green"));
    let reader = lexicon.freeze();

    let map_json = to_json(&map, &reader);
    let set_json = to_json(&set, &reader);

    let (rmap_lex, back_map): (LocalLexicon, SymMap<u32>) = from_json(&map_json);
    assert_eq!(back_map.len(), 2);
    let mut pairs: Vec<(&str, u32)> = back_map.iter().map(|(k, v)| (rmap_lex.resolve(*k), *v)).collect();
    pairs.sort_unstable();
    assert_eq!(pairs, [("one", 1), ("two", 2)]);

    let (rset_lex, back_set): (LocalLexicon, SymSet) = from_json(&set_json);
    let mut names: Vec<&str> = back_set.iter().map(|s| rset_lex.resolve(*s)).collect();
    names.sort_unstable();
    assert_eq!(names, ["green", "red"]);
}
