// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public API tests for arena-aware deserialization.

#![cfg(feature = "serde")]
#![allow(clippy::unwrap_used, reason = "test code")]

use std::sync::atomic::{AtomicUsize, Ordering};

use multitude::de::{DeserializationLimits, DeserializationResource, DeserializeIn, DeserializeInSeed, Entry, Number, Value};
use multitude::{Arc, Arena, Box, Cow, Rc};
use serde::Deserialize as _;
use serde::de::value::{
    BoolDeserializer, BorrowedBytesDeserializer, BytesDeserializer, Error as ValueError, MapDeserializer, SeqDeserializer, UnitDeserializer,
};
use serde::de::{DeserializeSeed as _, Deserializer, EnumAccess, IntoDeserializer, VariantAccess, Visitor};

extern crate multitude as renamed_multitude;

#[derive(Debug, DeserializeIn)]
struct __A {
    value: u64,
}

#[derive(DeserializeIn)]
struct __Serde {
    value: u64,
}

#[derive(Debug, DeserializeIn, PartialEq)]
#[serde(rename_all_fields = "SCREAMING_SNAKE_CASE")]
enum VariantFieldRename {
    #[serde(rename_all = "camelCase")]
    Record { field_name: u64 },
}

static ARRAY_DROP_MASK: AtomicUsize = AtomicUsize::new(0);

struct ArrayDropSpy(u8);

#[derive(Debug)]
struct ArrayTestError;

impl std::fmt::Display for ArrayTestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("array test error")
    }
}

impl std::error::Error for ArrayTestError {}

impl serde::de::Error for ArrayTestError {
    fn custom<T: std::fmt::Display>(_: T) -> Self {
        Self
    }
}

impl Drop for ArrayDropSpy {
    fn drop(&mut self) {
        ARRAY_DROP_MASK.fetch_or(1 << self.0, Ordering::Relaxed);
        assert_ne!(self.0, 0, "first array element panics during drop");
    }
}

impl<'de, A> DeserializeIn<'de, A> for ArrayDropSpy
where
    A: allocator_api2::alloc::Allocator + Clone,
{
    fn deserialize_in<D>(_: &Arena<A>, deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        u8::deserialize(deserializer).map(Self)
    }
}

#[derive(DeserializeIn)]
struct Metadata {
    active: bool,
    note: Option<Box<str>>,
}

#[derive(DeserializeIn)]
#[serde(deny_unknown_fields)]
struct Message {
    id: u64,
    #[serde(alias = "display_name")]
    name: Box<str>,
    tags: Box<[Box<str>]>,
    metadata: Metadata,
    #[multitude(via_serde)]
    source: std::string::String,
}

#[derive(serde::Deserialize, DeserializeIn)]
struct DualSerde {
    id: u64,
    #[multitude(via_serde)]
    label: std::string::String,
}

#[derive(DeserializeIn, Debug, PartialEq)]
enum Status {
    Ready,
    Failed(Box<str>),
    Progress { completed: u64, labels: Box<[Box<str>]> },
}

#[derive(DeserializeIn, Debug, PartialEq)]
enum CustomVariant {
    #[serde(deserialize_with = "deserialize_unit_variant")]
    Unit,
    #[serde(deserialize_with = "deserialize_pair_variant")]
    Pair(u64, u64),
    #[serde(deserialize_with = "deserialize_single_variant")]
    Single { value: u64 },
    #[serde(deserialize_with = "deserialize_named_variant")]
    Named { left: u64, right: u64 },
    #[serde(deserialize_with = "deserialize_opaque_variant")]
    Opaque(NoDeserializeIn),
    #[multitude(deserialize_with = "deserialize_adjusted_variant")]
    Adjusted(u64),
}

#[derive(Debug, PartialEq)]
struct NoDeserializeIn(u64);

fn deserialize_unit_variant<'de, D>(deserializer: D) -> Result<(), D::Error>
where
    D: Deserializer<'de>,
{
    serde::Deserialize::deserialize(deserializer)
}

fn deserialize_pair_variant<'de, D>(deserializer: D) -> Result<(u64, u64), D::Error>
where
    D: Deserializer<'de>,
{
    serde::Deserialize::deserialize(deserializer)
}

fn deserialize_single_variant<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    serde::Deserialize::deserialize(deserializer)
}

fn deserialize_named_variant<'de, D>(deserializer: D) -> Result<(u64, u64), D::Error>
where
    D: Deserializer<'de>,
{
    serde::Deserialize::deserialize(deserializer)
}

fn deserialize_opaque_variant<'de, D>(deserializer: D) -> Result<NoDeserializeIn, D::Error>
where
    D: Deserializer<'de>,
{
    u64::deserialize(deserializer).map(NoDeserializeIn)
}

fn deserialize_adjusted_variant<'de, A, D>(arena: &Arena<A>, deserializer: D) -> Result<u64, D::Error>
where
    A: allocator_api2::alloc::Allocator + Clone,
    D: Deserializer<'de>,
{
    u64::deserialize_in(arena, deserializer).map(|value| value + 10)
}

#[derive(DeserializeIn)]
struct Defaults<T> {
    #[serde(default)]
    value: T,
    #[multitude(skip)]
    skipped: u32,
}

#[derive(DeserializeIn)]
#[multitude(crate = "renamed_multitude")]
struct RenamedDependency {
    value: Box<str>,
}

#[derive(DeserializeIn, Debug, PartialEq)]
#[serde(default = "ordered_defaults", expecting = "an ordered record")]
struct OrderedDefaults {
    first: u64,
    #[serde(skip)]
    skipped: u64,
    trailing: u64,
    #[serde(default = "explicit_default")]
    explicit: u64,
}

fn ordered_defaults() -> OrderedDefaults {
    OrderedDefaults {
        first: 5,
        skipped: 99,
        trailing: 7,
        explicit: 88,
    }
}

const fn explicit_default() -> u64 {
    44
}

static CONTAINER_DEFAULT_CALLS: AtomicUsize = AtomicUsize::new(0);

#[derive(DeserializeIn)]
#[serde(default = "counted_container_default")]
struct ExplicitFieldDefaults {
    #[serde(default)]
    value: u64,
    #[serde(skip, default)]
    skipped: u64,
}

fn counted_container_default() -> ExplicitFieldDefaults {
    CONTAINER_DEFAULT_CALLS.fetch_add(1, Ordering::Relaxed);
    ExplicitFieldDefaults { value: 1, skipped: 2 }
}

#[derive(Debug, PartialEq)]
struct NoDefault(u64);

#[derive(DeserializeIn, Debug, PartialEq)]
#[serde(default = "skip_no_default_container")]
struct SkipWithoutDefault {
    value: u64,
    #[serde(skip)]
    skipped: NoDefault,
}

fn skip_no_default_container() -> SkipWithoutDefault {
    SkipWithoutDefault {
        value: 3,
        skipped: NoDefault(9),
    }
}

#[derive(DeserializeIn, Debug, PartialEq)]
enum OrderedVariant {
    Record {
        first: u64,
        #[serde(skip)]
        skipped: u64,
        #[serde(default)]
        trailing: u64,
    },
}

#[derive(DeserializeIn, Debug, PartialEq)]
struct OrdinalFields {
    first: u64,
    #[serde(skip)]
    skipped: u64,
    last: u64,
}

#[derive(DeserializeIn, Debug, PartialEq)]
enum OrdinalVariant {
    First,
    #[serde(skip)]
    #[expect(dead_code, reason = "the skipped variant verifies compact wire ordinals")]
    Hidden,
    Last,
}

#[derive(serde::Deserialize, Debug, PartialEq)]
struct ReplayedIdentifiers {
    first: u64,
    second: u64,
    third: u64,
}

struct EnumInput<K, P> {
    variant: K,
    payload: P,
}

impl<'de, K, P> Deserializer<'de> for EnumInput<K, P>
where
    K: IntoDeserializer<'de, ValueError>,
    P: Deserializer<'de, Error = ValueError>,
{
    type Error = ValueError;

    fn deserialize_any<W: Visitor<'de>>(self, visitor: W) -> Result<W::Value, Self::Error> {
        visitor.visit_enum(self)
    }

    fn deserialize_enum<W: Visitor<'de>>(
        self,
        _name: &'static str,
        _variants: &'static [&'static str],
        visitor: W,
    ) -> Result<W::Value, Self::Error> {
        visitor.visit_enum(self)
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
        bytes byte_buf option unit unit_struct newtype_struct seq tuple tuple_struct
        map struct identifier ignored_any
    }
}

impl<'de, K, P> EnumAccess<'de> for EnumInput<K, P>
where
    K: IntoDeserializer<'de, ValueError>,
    P: Deserializer<'de, Error = ValueError>,
{
    type Error = ValueError;
    type Variant = EnumPayload<P>;

    fn variant_seed<S: serde::de::DeserializeSeed<'de>>(self, seed: S) -> Result<(S::Value, Self::Variant), Self::Error> {
        let variant = seed.deserialize(self.variant.into_deserializer())?;
        Ok((variant, EnumPayload(self.payload)))
    }
}

struct EnumPayload<P>(P);

impl<'de, P> VariantAccess<'de> for EnumPayload<P>
where
    P: Deserializer<'de, Error = ValueError>,
{
    type Error = ValueError;

    fn unit_variant(self) -> Result<(), Self::Error> {
        <() as serde::Deserialize>::deserialize(self.0)
    }

    fn newtype_variant_seed<S: serde::de::DeserializeSeed<'de>>(self, seed: S) -> Result<S::Value, Self::Error> {
        seed.deserialize(self.0)
    }

    fn tuple_variant<W: Visitor<'de>>(self, _len: usize, visitor: W) -> Result<W::Value, Self::Error> {
        self.0.deserialize_any(visitor)
    }

    fn struct_variant<W: Visitor<'de>>(self, _fields: &'static [&'static str], visitor: W) -> Result<W::Value, Self::Error> {
        self.0.deserialize_any(visitor)
    }
}

#[test]
fn deserialize_builds_and_retains_an_arena_owned_graph() {
    let message: Arc<Message>;
    {
        let arena = Arena::new();
        let mut deserializer = serde_json::Deserializer::from_str(
            r#"{
                "id": 42,
                "display_name": "arena value",
                "tags": ["fast", "owned"],
                "metadata": {"active": true, "note": "nested"},
                "source": "ordinary serde"
            }"#,
        );

        message = arena.deserialize(&mut deserializer).unwrap();
        assert_eq!(message.id, 42);
    }

    assert_eq!(message.name.as_str(), "arena value");
    assert_eq!(message.tags.len(), 2);
    assert_eq!(message.tags[0].as_str(), "fast");
    assert_eq!(message.tags[1].as_str(), "owned");
    assert!(message.metadata.active);
    assert_eq!(message.metadata.note.as_deref(), Some("nested"));
    assert_eq!(message.source, "ordinary serde");
}

#[test]
fn deserialize_return_type_selects_the_recursive_root_path() {
    let arena = Arena::new();

    let mut box_deserializer =
        serde_json::Deserializer::from_str(r#"{"id":1,"name":"box","tags":[],"metadata":{"active":false,"note":null},"source":"test"}"#);
    let boxed: Box<Message> = arena.deserialize(&mut box_deserializer).unwrap();
    assert_eq!(boxed.name.as_str(), "box");

    let mut rc_deserializer =
        serde_json::Deserializer::from_str(r#"{"id":2,"name":"rc","tags":[],"metadata":{"active":true,"note":null},"source":"test"}"#);
    let rc: Rc<Message> = arena.deserialize(&mut rc_deserializer).unwrap();
    assert_eq!(rc.name.as_str(), "rc");
}

#[test]
fn ordinary_and_arena_deserialization_can_be_derived_together() {
    let ordinary: DualSerde = serde_json::from_str(r#"{"id":1,"label":"ordinary"}"#).unwrap();
    assert_eq!(ordinary.label, "ordinary");

    let arena = Arena::new();
    let arena_aware: DualSerde = arena.deserialize_json(r#"{"id":2,"label":"arena"}"#).unwrap();
    assert_eq!(arena_aware.id, 2);
    assert_eq!(arena_aware.label, "arena");
}

#[test]
fn deserialize_alloc_returns_an_arena_local_root() {
    let arena = Arena::new();
    let mut deserializer =
        serde_json::Deserializer::from_str(r#"{"id":3,"name":"alloc","tags":[],"metadata":{"active":true,"note":null},"source":"test"}"#);

    let local = arena.deserialize_alloc::<Message, _>(&mut deserializer).unwrap();
    assert_eq!(local.id, 3);
    assert_eq!(local.name.as_str(), "alloc");

    let limited = arena
        .deserialize_alloc_with_limits::<u64, _>(
            serde::de::value::U64Deserializer::<ValueError>::new(4),
            DeserializationLimits::unlimited(),
        )
        .unwrap();
    assert_eq!(*limited, 4);
}

#[test]
fn deserialize_externally_tagged_enum() {
    let arena = Arena::new();

    let mut unit = serde_json::Deserializer::from_str(r#""Ready""#);
    let status: Box<Status> = arena.deserialize(&mut unit).unwrap();
    assert_eq!(status.as_ref(), &Status::Ready);

    let mut newtype = serde_json::Deserializer::from_str(r#"{"Failed":"bad input"}"#);
    let status: Box<Status> = arena.deserialize(&mut newtype).unwrap();
    assert_eq!(status.as_ref(), &Status::Failed(arena.alloc_str_box("bad input")));

    let mut structure = serde_json::Deserializer::from_str(r#"{"Progress":{"completed":7,"labels":["a","b"]}}"#);
    let status: Box<Status> = arena.deserialize(&mut structure).unwrap();
    match &*status {
        Status::Progress { completed, labels } => {
            assert_eq!(*completed, 7);
            assert_eq!(labels[0].as_str(), "a");
            assert_eq!(labels[1].as_str(), "b");
        }
        _ => panic!("expected progress"),
    }
}

#[test]
fn deserialize_uses_variant_custom_deserializers() {
    let arena = Arena::new();

    let mut unit = serde_json::Deserializer::from_str(r#"{"Unit":null}"#);
    assert_eq!(CustomVariant::deserialize_in(&arena, &mut unit).unwrap(), CustomVariant::Unit);

    let mut pair = serde_json::Deserializer::from_str(r#"{"Pair":[3,4]}"#);
    assert_eq!(CustomVariant::deserialize_in(&arena, &mut pair).unwrap(), CustomVariant::Pair(3, 4));

    let mut single = serde_json::Deserializer::from_str(r#"{"Single":5}"#);
    assert_eq!(
        CustomVariant::deserialize_in(&arena, &mut single).unwrap(),
        CustomVariant::Single { value: 5 }
    );

    let mut named = serde_json::Deserializer::from_str(r#"{"Named":[6,7]}"#);
    assert_eq!(
        CustomVariant::deserialize_in(&arena, &mut named).unwrap(),
        CustomVariant::Named { left: 6, right: 7 }
    );

    let mut opaque = serde_json::Deserializer::from_str(r#"{"Opaque":8}"#);
    assert_eq!(
        CustomVariant::deserialize_in(&arena, &mut opaque).unwrap(),
        CustomVariant::Opaque(NoDeserializeIn(8))
    );

    let mut adjusted = serde_json::Deserializer::from_str(r#"{"Adjusted":5}"#);
    assert_eq!(
        CustomVariant::deserialize_in(&arena, &mut adjusted).unwrap(),
        CustomVariant::Adjusted(15)
    );
}

#[test]
fn deserialize_reports_structural_errors() {
    let arena = Arena::new();

    let mut missing = serde_json::Deserializer::from_str(r#"{"id":1,"tags":[],"metadata":{"active":true,"note":null},"source":"test"}"#);
    let result: Result<Arc<Message>, _> = arena.deserialize(&mut missing);
    let error = result.err().unwrap();
    assert!(error.to_string().contains("missing field"));

    let mut unknown = serde_json::Deserializer::from_str(
        r#"{"id":1,"name":"x","tags":[],"metadata":{"active":true,"note":null},"source":"test","extra":0}"#,
    );
    let result: Result<Arc<Message>, _> = arena.deserialize(&mut unknown);
    let error = result.err().unwrap();
    assert!(error.to_string().contains("unknown field"));
}

#[test]
fn deserialize_honors_defaults_skips_and_custom_crate_paths() {
    let arena = Arena::new();

    let mut defaults = serde_json::Deserializer::from_str("{}");
    let defaults: Box<Defaults<u64>> = arena.deserialize(&mut defaults).unwrap();
    assert_eq!(defaults.value, 0);
    assert_eq!(defaults.skipped, 0);

    let mut renamed = serde_json::Deserializer::from_str(r#"{"value":"renamed"}"#);
    let renamed: Box<RenamedDependency> = arena.deserialize(&mut renamed).unwrap();
    assert_eq!(renamed.value.as_str(), "renamed");
}

#[test]
fn named_structs_and_struct_variants_accept_ordered_sequences() {
    let arena = Arena::new();
    let ordered = OrderedDefaults::deserialize_in(&arena, SeqDeserializer::<_, ValueError>::new([11_u64].into_iter())).unwrap();
    assert_eq!(
        ordered,
        OrderedDefaults {
            first: 11,
            skipped: 99,
            trailing: 7,
            explicit: 44,
        }
    );

    let ordered = OrderedDefaults::deserialize_in(&arena, SeqDeserializer::<_, ValueError>::new([11_u64, 12, 13].into_iter())).unwrap();
    assert_eq!(
        ordered,
        OrderedDefaults {
            first: 11,
            skipped: 99,
            trailing: 12,
            explicit: 13,
        }
    );

    let mut empty_map = serde_json::Deserializer::from_str("{}");
    assert_eq!(
        OrderedDefaults::deserialize_in(&arena, &mut empty_map).unwrap(),
        OrderedDefaults {
            first: 5,
            skipped: 99,
            trailing: 7,
            explicit: 44,
        }
    );

    let variant = OrderedVariant::deserialize_in(
        &arena,
        EnumInput {
            variant: "Record",
            payload: SeqDeserializer::<_, ValueError>::new([21_u64, 22].into_iter()),
        },
    )
    .unwrap();
    assert_eq!(
        variant,
        OrderedVariant::Record {
            first: 21,
            skipped: 0,
            trailing: 22,
        }
    );

    assert_eq!(
        SkipWithoutDefault::deserialize_in(&arena, SeqDeserializer::<_, ValueError>::new([5_u64].into_iter())).unwrap(),
        SkipWithoutDefault {
            value: 5,
            skipped: NoDefault(9),
        }
    );

    CONTAINER_DEFAULT_CALLS.store(0, Ordering::Relaxed);
    let mut explicit = serde_json::Deserializer::from_str(r#"{"value":8}"#);
    let value = ExplicitFieldDefaults::deserialize_in(&arena, &mut explicit).unwrap();
    assert_eq!((value.value, value.skipped), (8, 0));
    assert_eq!(CONTAINER_DEFAULT_CALLS.load(Ordering::Relaxed), 1);
}

#[test]
fn derive_accepts_compact_numeric_field_and_variant_ordinals() {
    let arena = Arena::new();
    let fields = OrdinalFields::deserialize_in(
        &arena,
        MapDeserializer::<_, ValueError>::new([(0_u64, 10_u64), (1, 20)].into_iter()),
    )
    .unwrap();
    assert_eq!(
        fields,
        OrdinalFields {
            first: 10,
            skipped: 0,
            last: 20,
        }
    );

    let variant = OrdinalVariant::deserialize_in(
        &arena,
        EnumInput {
            variant: 1_u64,
            payload: UnitDeserializer::<ValueError>::new(),
        },
    )
    .unwrap();
    assert_eq!(variant, OrdinalVariant::Last);

    let fields = OrdinalFields::deserialize_in(
        &arena,
        MapDeserializer::<_, ValueError>::new([(0_u64, 10_u64), (2, 99), (1, 20)].into_iter()),
    )
    .unwrap();
    assert_eq!(
        fields,
        OrdinalFields {
            first: 10,
            skipped: 0,
            last: 20,
        }
    );

    let error = OrdinalVariant::deserialize_in(
        &arena,
        EnumInput {
            variant: 2_u64,
            payload: UnitDeserializer::<ValueError>::new(),
        },
    )
    .unwrap_err();
    assert!(error.to_string().contains("variant index 0 <= i < 2"));
}

#[test]
fn value_replays_string_byte_and_numeric_identifiers() {
    let arena = Arena::new();
    let value = Value::Map(arena.alloc_slice_fill_iter_box([
        Entry {
            key: Value::String(arena.alloc_str_box("first")),
            value: Value::Number(Number::U64(1)),
        },
        Entry {
            key: Value::Bytes(arena.alloc_slice_copy_box(b"second")),
            value: Value::Number(Number::U64(2)),
        },
        Entry {
            key: Value::Number(Number::U64(2)),
            value: Value::Number(Number::U64(3)),
        },
    ]));

    assert_eq!(
        ReplayedIdentifiers::deserialize(&value).unwrap(),
        ReplayedIdentifiers {
            first: 1,
            second: 2,
            third: 3,
        }
    );

    let sequence = Value::Sequence(arena.alloc_slice_fill_iter_box([Value::Number(Number::U64(4)), Value::Number(Number::U64(5))]));
    assert_eq!(
        OrdinalFields::deserialize_in(&arena, &sequence).unwrap(),
        OrdinalFields {
            first: 4,
            skipped: 0,
            last: 5,
        }
    );

    let numeric_variant = Value::Map(arena.alloc_slice_fill_iter_box([Entry {
        key: Value::Number(Number::U64(1)),
        value: Value::Unit,
    }]));
    assert_eq!(
        OrdinalVariant::deserialize_in(&arena, &numeric_variant).unwrap(),
        OrdinalVariant::Last
    );

    let struct_variant = Value::Map(arena.alloc_slice_fill_iter_box([Entry {
        key: Value::String(arena.alloc_str_box("Record")),
        value: Value::Sequence(arena.alloc_slice_fill_iter_box([Value::Number(Number::U64(6)), Value::Number(Number::U64(7))])),
    }]));
    assert_eq!(
        OrderedVariant::deserialize_in(&arena, &struct_variant).unwrap(),
        OrderedVariant::Record {
            first: 6,
            skipped: 0,
            trailing: 7,
        }
    );
}

#[test]
fn custom_container_expecting_text_is_reported() {
    let arena = Arena::new();
    let error = OrderedDefaults::deserialize_in(&arena, BoolDeserializer::<ValueError>::new(true)).unwrap_err();
    assert!(error.to_string().contains("an ordered record"));
}

#[test]
fn deny_unknown_fields_rejects_skipped_field_input() {
    #[derive(DeserializeIn)]
    #[serde(deny_unknown_fields)]
    #[expect(dead_code, reason = "the skipped field exists to test input rejection")]
    struct Strict {
        #[serde(skip)]
        ignored: u64,
    }

    let arena = Arena::new();
    let mut deserializer = serde_json::Deserializer::from_str(r#"{"ignored":1}"#);
    let result: Result<Box<Strict>, _> = arena.deserialize(&mut deserializer);
    let error = result.err().unwrap();
    assert!(error.to_string().contains("unknown field"));

    let error = Strict::deserialize_in(&arena, MapDeserializer::<_, ValueError>::new([(0_u64, 1_u64)].into_iter()))
        .err()
        .expect("numeric field must be rejected");
    assert!(error.to_string().contains("field index 0 <= i < 0"));
}

#[test]
fn cow_str_borrows_unescaped_input_and_owns_decoded_input() {
    let arena = Arena::new();

    let mut borrowed_deserializer = serde_json::Deserializer::from_slice(br#""borrowed""#);
    let borrowed: Cow<'_, str> = DeserializeIn::deserialize_in(&arena, &mut borrowed_deserializer).unwrap();
    assert!(borrowed.is_borrowed());
    assert_eq!(&*borrowed, "borrowed");

    let mut owned_deserializer = serde_json::Deserializer::from_slice(br#""decoded\u0020value""#);
    let owned: Cow<'_, str> = DeserializeIn::deserialize_in(&arena, &mut owned_deserializer).unwrap();
    assert!(!owned.is_borrowed());
    assert_eq!(&*owned, "decoded value");
}

#[test]
fn cow_bytes_borrows_or_owns_as_exposed_by_the_deserializer() {
    let arena = Arena::new();
    let borrowed: Cow<'_, [u8]> = arena
        .deserialize(BorrowedBytesDeserializer::<ValueError>::new(b"borrowed"))
        .unwrap();
    assert!(borrowed.is_borrowed());
    assert_eq!(&*borrowed, b"borrowed");

    let owned: Cow<'_, [u8]> = arena.deserialize(BytesDeserializer::<ValueError>::new(b"copied")).unwrap();
    assert!(owned.is_owned());
    assert_eq!(&*owned, b"copied");
}

#[test]
fn deserialize_reusing_reuses_string_and_vector_capacity() {
    let arena = Arena::new();

    let mut string = arena.alloc_string_with_capacity(32);
    string.push_str("existing allocation");
    let string_ptr = string.as_ptr();
    let mut string_deserializer = serde_json::Deserializer::from_str(r#""replacement""#);
    string.deserialize_reusing(&mut string_deserializer).unwrap();
    assert_eq!(string.as_str(), "replacement");
    assert_eq!(string.as_ptr(), string_ptr);

    let mut values = arena.alloc_vec_with_capacity(8);
    values.extend([10_u64, 20, 30]);
    let values_ptr = values.as_ptr();
    let mut values_deserializer = serde_json::Deserializer::from_str("[1,2,3,4]");
    values.deserialize_reusing(&mut values_deserializer).unwrap();
    assert_eq!(values.as_slice(), &[1, 2, 3, 4]);
    assert_eq!(values.as_ptr(), values_ptr);
}

#[test]
fn deserialize_reusing_leaves_a_valid_partial_value_on_error() {
    let arena = Arena::new();
    let mut values = arena.alloc_vec_with_capacity(4);
    values.extend([10_u64, 20]);

    let mut deserializer = serde_json::Deserializer::from_str("[1,2,\"invalid\"]");
    assert!(values.deserialize_reusing(&mut deserializer).is_err());
    assert_eq!(values.as_slice(), &[1, 2]);
}

#[test]
fn deserialize_json_reusing_checks_complete_input_and_retains_capacity() {
    let arena = Arena::new();
    let mut values = arena.alloc_vec_with_capacity::<u64>(8);
    values.extend([10, 20, 30]);
    let pointer = values.as_ptr();

    values.deserialize_json_reusing("[1,2,3,4]").unwrap();
    assert_eq!(values.as_slice(), &[1, 2, 3, 4]);
    assert_eq!(values.as_ptr(), pointer);

    let error = values.deserialize_json_reusing("[5,6] trailing").unwrap_err();
    assert!(error.to_string().contains("trailing characters"));
    assert_eq!(values.as_slice(), &[5, 6]);
    assert_eq!(values.as_ptr(), pointer);
}

#[test]
fn deserialize_json_each_streams_in_order_and_allows_selective_retention() {
    #[derive(DeserializeIn)]
    struct Item {
        id: u64,
        label: Box<str>,
    }

    let retained = {
        let arena = Arena::new();
        let mut seen = std::vec::Vec::new();
        let mut retained = std::vec::Vec::new();

        arena
            .deserialize_json_each(
                r#"[{"id":1,"label":"discard"},{"id":2,"label":"retain"},{"id":3,"label":"discard"}]"#,
                |item: Item| {
                    seen.push(item.id);
                    if item.id.is_multiple_of(2) {
                        retained.push(item.label);
                    }
                },
            )
            .unwrap();

        assert_eq!(seen, [1, 2, 3]);
        retained
    };

    assert_eq!(retained.len(), 1);
    assert_eq!(retained[0].as_str(), "retain");
}

#[cfg(feature = "serde_json")]
#[test]
fn deserialize_json_each_streams_borrowed_raw_values_without_materializing_them() {
    let arena = Arena::new();
    let input = r#"[{"id":1,"payload":"discard\u0020me"},{"id":2,"payload":"retain"}]"#;
    let mut values = std::vec::Vec::new();

    arena
        .deserialize_json_each(input, |value: &serde_json::value::RawValue| values.push(value.get()))
        .unwrap();

    assert_eq!(
        values,
        [r#"{"id":1,"payload":"discard\u0020me"}"#, r#"{"id":2,"payload":"retain"}"#]
    );
    let input_range = input.as_bytes().as_ptr_range();
    assert!(values.iter().all(|value| input_range.contains(&value.as_ptr())));
}

#[test]
fn deserialize_json_each_reports_errors_after_the_processed_prefix() {
    let arena = Arena::new();
    let mut values = std::vec::Vec::new();

    let error = arena
        .deserialize_json_each("[1,2,\"invalid\"]", |value: u64| values.push(value))
        .unwrap_err();
    assert!(error.to_string().contains("invalid type"));
    assert_eq!(values, [1, 2]);

    let error = arena
        .deserialize_json_each("[3,4] trailing", |value: u64| values.push(value))
        .unwrap_err();
    assert!(error.to_string().contains("trailing characters"));
    assert_eq!(values, [1, 2, 3, 4]);

    let error = arena.deserialize_json_each("5", |_: u64| {}).unwrap_err();
    assert!(error.to_string().contains("expected a sequence"));
}

#[test]
fn deserialize_json_each_with_limits_bounds_the_top_level_sequence() {
    let arena = Arena::new();
    let limits = DeserializationLimits::unlimited().with_max_sequence_len(2);
    let mut values = std::vec::Vec::new();

    let error = arena
        .deserialize_json_each_with_limits("[1,2,3]", limits, |value: u64| values.push(value))
        .unwrap_err();
    let exceeded = error.limit_exceeded().unwrap();
    assert_eq!(exceeded.resource(), DeserializationResource::SequenceLength);
    assert_eq!(exceeded.limit(), 2);
    assert_eq!(values, [1, 2]);

    let error = arena
        .deserialize_json_each_with_limits("[3] trailing", DeserializationLimits::unlimited(), |value: u64| values.push(value))
        .unwrap_err();
    assert!(!error.is_limit_exceeded());
    assert!(error.as_json_error().to_string().contains("trailing characters"));
    assert_eq!(values, [1, 2, 3]);
}

#[test]
fn deserialize_json_each_preserves_borrowed_and_owned_cow_storage() {
    let arena = Arena::new();
    let input = r#"["borrowed","owned\u0020value"]"#;
    let mut values = std::vec::Vec::new();

    arena
        .deserialize_json_each(input, |value: Cow<'_, str>| values.push(value))
        .unwrap();

    assert!(values[0].is_borrowed());
    assert!(values[1].is_owned());
    assert_eq!(&*values[0], "borrowed");
    assert_eq!(&*values[1], "owned value");
}

#[test]
fn deserialize_json_reusing_with_limits_rejects_oversized_sequence() {
    let arena = Arena::new();
    let mut values = arena.alloc_vec::<u64>();
    let limits = DeserializationLimits::unlimited().with_max_sequence_len(2);

    let error = values.deserialize_json_reusing_with_limits("[1,2,3]", limits).unwrap_err();
    let exceeded = error.limit_exceeded().unwrap();
    assert_eq!(exceeded.resource(), DeserializationResource::SequenceLength);
    assert_eq!(exceeded.limit(), 2);
    assert_eq!(values.as_slice(), &[1, 2]);
}

#[test]
fn deserialize_json_reusing_with_limits_checks_complete_input() {
    let arena = Arena::new();
    let mut values = arena.alloc_vec_with_capacity::<u64>(4);
    let pointer = values.as_ptr();
    let limits = DeserializationLimits::unlimited().with_max_sequence_len(3);

    values.deserialize_json_reusing_with_limits("[1,2,3]", limits).unwrap();
    assert_eq!(values.as_slice(), &[1, 2, 3]);
    assert_eq!(values.as_ptr(), pointer);

    let error = values.deserialize_json_reusing_with_limits("[4,5] trailing", limits).unwrap_err();
    assert!(error.as_json_error().to_string().contains("trailing characters"));
    assert_eq!(values.as_slice(), &[4, 5]);
    assert_eq!(values.as_ptr(), pointer);
}

#[test]
fn deserialize_reusing_with_limits_preserves_the_accepted_prefix() {
    let arena = Arena::new();
    let mut values = arena.alloc_vec::<u64>();
    let limits = DeserializationLimits::unlimited().with_max_sequence_len(2);
    let input = SeqDeserializer::<_, ValueError>::new([1_u64, 2, 3].into_iter());

    let error = values.deserialize_reusing_with_limits(input, limits).unwrap_err();
    assert!(error.to_string().contains("sequence length limit"));
    assert_eq!(values.as_slice(), &[1, 2]);
}

#[test]
fn recursive_standard_containers_use_arena_aware_elements() {
    let arena = Arena::new();
    let mut deserializer = serde_json::Deserializer::from_str(r#"["arena",[1,2,3]]"#);
    let value = DeserializeInSeed::<(Box<str>, [u64; 3]), _>::new(&arena)
        .deserialize(&mut deserializer)
        .unwrap();
    assert_eq!(value.0.as_str(), "arena");
    assert_eq!(value.1, [1, 2, 3]);

    let mut result_deserializer = serde_json::Deserializer::from_str(r#"{"Ok":"value"}"#);
    let result = DeserializeInSeed::<Result<Box<str>, Box<str>>, _>::new(&arena)
        .deserialize(&mut result_deserializer)
        .unwrap();
    assert_eq!(result.unwrap().as_str(), "value");

    let mut map_deserializer = serde_json::Deserializer::from_str(r#"{"first":"arena","second":"values"}"#);
    let map = DeserializeInSeed::<std::collections::BTreeMap<Box<str>, Box<str>>, _>::new(&arena)
        .deserialize(&mut map_deserializer)
        .unwrap();
    assert_eq!(map["first"].as_str(), "arena");
}

#[test]
fn deserialization_limits_reject_oversized_values() {
    let arena = Arena::new();
    let limits = DeserializationLimits::unlimited().with_max_sequence_len(2).with_max_string_len(4);

    let error = arena.deserialize_json_with_limits::<Box<[u64]>, _>("[1,2,3]", limits).unwrap_err();
    let exceeded = error.limit_exceeded().unwrap();
    assert_eq!(exceeded.resource(), DeserializationResource::SequenceLength);
    assert_eq!(exceeded.limit(), 2);

    let error = arena.deserialize_json_with_limits::<Box<str>, _>(r#""12345""#, limits).unwrap_err();
    let exceeded = error.limit_exceeded().unwrap();
    assert_eq!(exceeded.resource(), DeserializationResource::StringLength);
    assert_eq!(exceeded.limit(), 4);
}

#[test]
fn limited_json_errors_classify_resources_and_preserve_json_errors() {
    use std::error::Error as _;

    let arena = Arena::new();

    let depth = DeserializationLimits::unlimited().with_max_depth(0);
    let error = arena.deserialize_json_with_limits::<Box<[u64]>, _>("[1]", depth).unwrap_err();
    assert!(error.is_limit_exceeded());
    assert_eq!(error.limit_exceeded().unwrap().resource(), DeserializationResource::Depth);
    assert!(error.to_string().contains("nesting depth limit"));

    let map = DeserializationLimits::unlimited().with_max_map_len(0);
    let error = arena.deserialize_json_with_limits::<Value, _>(r#"{"key":1}"#, map).unwrap_err();
    assert_eq!(error.limit_exceeded().unwrap().resource(), DeserializationResource::MapLength);
    assert!(error.to_string().contains("map length limit"));

    let string = DeserializationLimits::unlimited().with_max_string_len(0);
    let error = arena.deserialize_json_with_limits::<Box<str>, _>(r#""value""#, string).unwrap_err();
    assert_eq!(error.limit_exceeded().unwrap().resource(), DeserializationResource::StringLength);
    assert!(error.to_string().contains("string length limit"));

    let bytes = DeserializationLimits::unlimited().with_max_bytes_len(0);
    let error = arena
        .deserialize_json_with_limits::<Cow<'_, [u8]>, _>(r#""value""#, bytes)
        .unwrap_err();
    assert_eq!(
        error.limit_exceeded().unwrap().resource(),
        DeserializationResource::ByteStringLength
    );
    assert!(error.to_string().contains("byte string length limit"));

    let error = arena
        .deserialize_json_with_limits::<Box<u64>, _>("invalid", DeserializationLimits::unlimited())
        .unwrap_err();
    assert!(!error.is_limit_exceeded());
    assert!(error.as_json_error().is_syntax());
    let _ = error.backtrace();
    assert!(error.source().is_some());
    assert_eq!(error.to_string(), "JSON deserialization failed");
    assert!(error.source().unwrap().to_string().contains("expected value"));
}

#[test]
#[cfg_attr(miri, ignore)] // std backtrace capture calls readlink, unsupported under Miri isolation
fn limited_json_error_captures_an_enabled_backtrace() {
    const CHILD: &str = "MULTITUDE_BACKTRACE_TEST_CHILD";

    if std::env::var_os(CHILD).is_some() {
        let arena = Arena::new();
        let error = arena
            .deserialize_json_with_limits::<Box<u64>, _>("invalid", DeserializationLimits::unlimited())
            .unwrap_err();
        assert_eq!(error.backtrace().status(), std::backtrace::BacktraceStatus::Captured);
        return;
    }

    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .arg("--exact")
        .arg("limited_json_error_captures_an_enabled_backtrace")
        .env(CHILD, "1")
        .env("RUST_BACKTRACE", "1")
        .status()
        .unwrap();
    assert!(status.success());
}

#[cfg(feature = "serde_json")]
#[test]
fn deserialization_limits_apply_to_ignored_unknown_fields() {
    #[derive(Debug, DeserializeIn)]
    #[expect(dead_code, reason = "deserialization is expected to fail while skipping the unknown field")]
    struct KnownField {
        kept: u64,
    }

    let arena = Arena::new();
    let limits = DeserializationLimits::unlimited().with_max_sequence_len(0);
    let error = arena
        .deserialize_json_with_limits::<Box<KnownField>, _>(r#"{"kept":1,"ignored":[2]}"#, limits)
        .unwrap_err();

    assert!(error.to_string().contains("sequence length limit"));
}

#[test]
fn derive_internal_names_and_variant_field_renaming_are_hygienic() {
    let arena = Arena::new();

    let mut allocator_name = serde_json::Deserializer::from_str(r#"{"value":1}"#);
    let value = __A::deserialize_in(&arena, &mut allocator_name).unwrap();
    assert_eq!(value.value, 1);

    let mut serde_name = serde_json::Deserializer::from_str(r#"{"value":2}"#);
    let value = __Serde::deserialize_in(&arena, &mut serde_name).unwrap();
    assert_eq!(value.value, 2);

    let mut renamed = serde_json::Deserializer::from_str(r#"{"Record":{"fieldName":3}}"#);
    let value = VariantFieldRename::deserialize_in(&arena, &mut renamed).unwrap();
    assert_eq!(value, VariantFieldRename::Record { field_name: 3 });
}

#[cfg(feature = "serde_json")]
#[test]
fn json_convenience_apis_reject_trailing_input() {
    let arena = Arena::new();
    let value: Box<Box<str>> = arena.deserialize_json(br#""value""#).unwrap();
    assert_eq!(value.as_str(), "value");

    let result: serde_json::Result<Box<Box<str>>> = arena.deserialize_json(br#""value" null"#);
    let error = result.unwrap_err();
    assert!(error.to_string().contains("trailing characters"));

    let local = arena.deserialize_json_alloc::<Box<str>, _>(br#""local""#).unwrap();
    assert_eq!(local.as_str(), "local");

    let result = arena.deserialize_json_alloc::<Box<str>, _>(br#""local" null"#);
    assert!(result.unwrap_err().to_string().contains("trailing characters"));

    let local = arena
        .deserialize_json_alloc_with_limits::<Box<str>, _>(br#""limited""#, DeserializationLimits::unlimited())
        .unwrap();
    assert_eq!(local.as_str(), "limited");

    let limits = DeserializationLimits::unlimited().with_max_string_len(4);
    let result = arena.deserialize_json_alloc_with_limits::<Box<str>, _>(br#""oversized""#, limits);
    let exceeded = result.unwrap_err().limit_exceeded().unwrap();
    assert_eq!(exceeded.resource(), DeserializationResource::StringLength);
    assert_eq!(exceeded.limit(), 4);
}

#[test]
fn dynamic_value_preserves_maps_and_replays_through_serde() {
    let arena = Arena::new();
    let mut map_deserializer = serde_json::Deserializer::from_str(r#"{"key":1,"key":2}"#);
    let value: Value = DeserializeIn::deserialize_in(&arena, &mut map_deserializer).unwrap();
    assert_eq!(value.get_all("key").count(), 2);

    let mut tuple_deserializer = serde_json::Deserializer::from_str(r#"["text",42]"#);
    let tuple: Value = DeserializeIn::deserialize_in(&arena, &mut tuple_deserializer).unwrap();
    let replayed = <(std::string::String, u64) as serde::Deserialize>::deserialize(&tuple).unwrap();
    assert_eq!(replayed, ("text".to_owned(), 42));

    let mut float_deserializer = serde_json::Deserializer::from_str("1.5");
    let float: Value = DeserializeIn::deserialize_in(&arena, &mut float_deserializer).unwrap();
    assert_eq!(
        <f32 as serde::Deserialize>::deserialize(&float).unwrap().to_bits(),
        1.5_f32.to_bits()
    );

    let mut char_deserializer = serde_json::Deserializer::from_str(r#""x""#);
    let character: Value = DeserializeIn::deserialize_in(&arena, &mut char_deserializer).unwrap();
    assert_eq!(<char as serde::Deserialize>::deserialize(&character).unwrap(), 'x');
}

#[test]
fn array_cleanup_drops_remaining_elements_when_one_drop_panics() {
    ARRAY_DROP_MASK.store(0, Ordering::Relaxed);
    let arena = Arena::new();

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let deserializer = SeqDeserializer::<_, ArrayTestError>::new([0_u8, 1].into_iter());
        let _: Result<[ArrayDropSpy; 3], _> = DeserializeIn::deserialize_in(&arena, deserializer);
    }));

    assert!(result.is_err());
    assert_eq!(ARRAY_DROP_MASK.load(Ordering::Relaxed), 0b11);
}

#[cfg(feature = "serde_json")]
#[test]
#[cfg_attr(miri, ignore)]
fn arbitrary_json_strings_round_trip_into_arena_storage() {
    bolero::check!()
        .with_type::<std::string::String>()
        .for_each(|input: &std::string::String| {
            let encoded = serde_json::to_vec(input).unwrap();
            let arena = Arena::new();
            let value: Box<Box<str>> = arena.deserialize_json(&encoded).unwrap();
            assert_eq!(value.as_str(), input);
        });
}

#[cfg(feature = "serde_json")]
#[test]
#[cfg_attr(miri, ignore)]
fn arbitrary_input_does_not_break_dynamic_value_invariants() {
    bolero::check!()
        .with_type::<std::vec::Vec<u8>>()
        .for_each(|input: &std::vec::Vec<u8>| {
            let arena = Arena::new();
            let mut deserializer = serde_json::Deserializer::from_slice(input);
            let _ = <Value as DeserializeIn<'_, _>>::deserialize_in(&arena, &mut deserializer);
        });
}
