// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Query codec integration tests.

use std::borrow::Cow;
use std::fmt;
use std::marker::PhantomData;
use std::num::ParseIntError;
use std::str::FromStr;

use routerama::query::{Error, ErrorKind, FromQuery, QueryLimits, ToQuery};

#[derive(Debug, PartialEq, Eq, FromQuery, ToQuery)]
#[query(deny_unknown_fields)]
struct SearchQuery<'q> {
    q: Cow<'q, str>,
    page: Option<u32>,
    #[query(alias = "tags")]
    tag: Vec<Cow<'q, str>>,
    #[query(default)]
    order: String,
}

#[derive(Debug, PartialEq, Eq, routerama::query::FromQuery, routerama::query::ToQuery)]
struct Paging {
    page: u32,
    #[query(default)]
    per_page: u32,
}

#[derive(Debug, PartialEq, Eq, routerama::query::FromQuery, routerama::query::ToQuery)]
struct Flattened<'q> {
    q: &'q str,
    #[query(flatten)]
    paging: Paging,
}

#[derive(Debug, PartialEq, Eq, routerama::query::FromQuery, routerama::query::ToQuery)]
#[query(rename_all = "camelCase")]
struct Renamed<'q> {
    user_name: &'q str,
    #[query(rename = "max", alias = "limit")]
    maximum: u32,
}

#[derive(Debug, PartialEq, Eq, routerama::query::FromQuery, routerama::query::ToQuery)]
struct NumericQuery {
    unsigned8: u8,
    unsigned16: u16,
    unsigned32: u32,
    unsigned64: u64,
    unsigned128: u128,
    pointer_unsigned: usize,
    signed8: i8,
    signed16: i16,
    signed32: i32,
    signed64: i64,
    signed128: i128,
    pointer_signed: isize,
    enabled: bool,
}

#[derive(Debug, PartialEq, Eq)]
struct DisplayValue(&'static str);

impl fmt::Display for DisplayValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

struct FailingDisplay;

impl fmt::Display for FailingDisplay {
    fn fmt(&self, _formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        Err(fmt::Error)
    }
}

#[derive(routerama::query::ToQuery)]
struct DisplayQuery {
    value: DisplayValue,
}

#[derive(routerama::query::ToQuery)]
struct FailingQuery {
    value: FailingDisplay,
}

#[derive(Debug)]
struct PrivateField(u32);

impl FromStr for PrivateField {
    type Err = ParseIntError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        value.parse().map(Self)
    }
}

/// Public query type whose field type remains private.
#[derive(Debug, routerama::query::FromQuery)]
pub struct PublicQueryWithPrivateField {
    value: PrivateField,
}

struct RejectingWriter;

impl fmt::Write for RejectingWriter {
    fn write_str(&mut self, _value: &str) -> fmt::Result {
        Err(fmt::Error)
    }
}

#[derive(Debug, PartialEq, Eq, FromQuery, ToQuery)]
struct GenericNested<T> {
    nested: T,
}

#[derive(Debug, PartialEq, Eq, FromQuery, ToQuery)]
struct GenericQuery<T, const N: usize>
where
    T: Clone,
{
    scalar: T,
    optional: Option<T>,
    values: Vec<T>,
    #[query(flatten)]
    child: GenericNested<T>,
    #[query(default)]
    fallback: T,
    #[query(skip)]
    marker: PhantomData<[T; N]>,
}

#[derive(Debug, PartialEq, Eq, FromQuery, ToQuery)]
struct MultipleLifetimes<'q, 'a, 'b> {
    borrowed: &'q str,
    #[query(skip)]
    first: PhantomData<&'a ()>,
    #[query(skip)]
    second: PhantomData<&'b ()>,
}

#[derive(ToQuery)]
struct MultiLifetimeEncoder<'a, 'b> {
    first: &'a str,
    second: &'b str,
}

#[derive(Debug, PartialEq, Eq, FromQuery)]
struct GeneratedNameCollision<'__routerama_q, __RouteramaFlattenDecoder0> {
    #[query(flatten)]
    nested: __RouteramaFlattenDecoder0,
    #[query(skip)]
    marker: PhantomData<&'__routerama_q ()>,
}

#[derive(Debug, PartialEq, Eq, FromQuery, ToQuery)]
struct DefaultedGeneric<T = GenericNested<u32>> {
    #[query(flatten)]
    nested: T,
}

#[derive(Debug, PartialEq, Eq, FromQuery)]
struct HigherRankedLifetime<T>
where
    T: for<'__routerama_q> Fn(&'__routerama_q str),
{
    value: String,
    #[query(skip)]
    marker: PhantomData<T>,
}

#[derive(Debug, PartialEq, Eq)]
struct ConstDefault<const N: usize>;

impl Default for ConstDefault<4> {
    fn default() -> Self {
        Self
    }
}

#[derive(Debug, PartialEq, Eq)]
struct LifetimeDefault<'a>(PhantomData<&'a ()>);

impl Default for LifetimeDefault<'static> {
    fn default() -> Self {
        Self(PhantomData)
    }
}

#[derive(Debug, PartialEq, Eq, FromQuery)]
struct GenericDefaultBounds<'a, const N: usize> {
    value: String,
    #[query(skip)]
    const_marker: ConstDefault<N>,
    #[query(skip)]
    lifetime_marker: LifetimeDefault<'a>,
}

#[derive(Debug, PartialEq, Eq, FromQuery)]
#[query(deny_unknown_fields)]
struct StrictLeft {
    left: Option<u32>,
}

#[derive(Debug, PartialEq, Eq, FromQuery)]
struct Right {
    right: Option<u32>,
}

#[derive(Debug, PartialEq, Eq, FromQuery)]
struct StrictThenRight {
    #[query(flatten)]
    strict: StrictLeft,
    #[query(flatten)]
    right: Right,
}

#[derive(Debug, PartialEq, Eq, FromQuery)]
struct RightThenStrict {
    #[query(flatten)]
    right: Right,
    #[query(flatten)]
    strict: StrictLeft,
}

#[derive(Debug, PartialEq, Eq, FromQuery)]
#[query(deny_unknown_fields)]
struct StrictOuter {
    #[query(flatten)]
    strict: StrictLeft,
    #[query(flatten)]
    right: Right,
}

#[derive(Debug, PartialEq, Eq, FromQuery)]
struct SharedOne {
    shared: Option<u32>,
}

#[derive(Debug, PartialEq, Eq, FromQuery)]
struct SharedTwo {
    shared: Option<u32>,
}

#[derive(Debug, PartialEq, Eq, FromQuery)]
struct AmbiguousFlatten {
    #[query(flatten)]
    one: SharedOne,
    #[query(flatten)]
    two: SharedTwo,
}

#[derive(Debug, PartialEq, Eq, FromQuery)]
struct DirectPrecedence {
    shared: u32,
    #[query(flatten)]
    one: SharedOne,
    #[query(flatten)]
    two: SharedTwo,
}

mod custom_names {
    use std::fmt;
    use std::str::FromStr;

    #[derive(Debug, PartialEq, Eq)]
    pub(crate) struct String(pub(crate) u32);

    #[derive(Debug, PartialEq, Eq)]
    pub(crate) struct Option<T>(pub(crate) T);

    #[derive(Debug, PartialEq, Eq)]
    pub(crate) struct Vec<T>(pub(crate) T);

    macro_rules! impl_codec {
        ($wrapper:ident $(<$generic:ident>)?) => {
            impl$(<$generic>)? FromStr for $wrapper$(<$generic>)?
            where
                $($generic: FromStr,)?
                $(<$generic as FromStr>::Err: fmt::Debug,)?
            {
                type Err = std::string::String;

                fn from_str(value: &str) -> Result<Self, Self::Err> {
                    value.parse().map(Self).map_err(|error| format!("{error:?}"))
                }
            }

            impl$(<$generic>)? fmt::Display for $wrapper$(<$generic>)?
            where
                $($generic: fmt::Display,)?
            {
                fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                    self.0.fmt(formatter)
                }
            }
        };
    }

    impl_codec!(String);
    impl_codec!(Option<T>);
    impl_codec!(Vec<T>);
}

#[derive(Debug, PartialEq, Eq, FromQuery, ToQuery)]
struct SpellingQuery<String> {
    generic: String,
    custom_string: custom_names::String,
    custom_option: custom_names::Option<u32>,
    custom_vec: custom_names::Vec<u32>,
    standard_string: std::string::String,
    standard_option: core::option::Option<u32>,
    standard_vec: std::vec::Vec<u32>,
}

trait OnlyOuterState {}

#[derive(Debug, PartialEq, Eq, FromQuery, ToQuery)]
struct State<W>
where
    Self: OnlyOuterState,
{
    value: W,
}

impl<W> OnlyOuterState for State<W> {}

#[derive(Debug, PartialEq, Eq, FromQuery)]
struct DefaultsOnContainers {
    #[query(default)]
    optional: Option<u32>,
    #[serde(default)]
    repeated: Vec<u32>,
}

#[derive(FromQuery)]
struct SkippedGenericCow<'a, B>
where
    B: ToOwned + ?Sized,
{
    value: String,
    #[query(skip)]
    skipped: Cow<'a, B>,
}

#[test]
fn default_limits_are_pinned() {
    assert_eq!(QueryLimits::DEFAULT.max_query_length, 16_384);
    assert_eq!(QueryLimits::DEFAULT.max_pairs, 256);
    assert_eq!(QueryLimits::DEFAULT.max_decoded_length, 65_536);
    assert_eq!(QueryLimits::DEFAULT.max_repeated_values, 256);
    assert_eq!(QueryLimits::DEFAULT.max_encoded_length, 65_536);
}

#[test]
fn public_query_types_can_contain_private_field_types() {
    let parsed = PublicQueryWithPrivateField::from_query("value=42").expect("private field type parses");
    assert_eq!(parsed.value.0, 42);
}

#[test]
fn root_imports_and_generic_query_structs_support_all_generic_kinds() {
    let parsed = GenericQuery::<u32, 4>::from_query("scalar=1&optional=2&values=3&values=4&nested=5").expect("generic query parses");
    assert_eq!(
        parsed,
        GenericQuery {
            scalar: 1,
            optional: Some(2),
            values: vec![3, 4],
            child: GenericNested { nested: 5 },
            fallback: 0,
            marker: PhantomData,
        }
    );
    assert_eq!(
        parsed.to_query_string().expect("generic query encodes"),
        "scalar=1&optional=2&values=3&values=4&nested=5&fallback=0"
    );
}

#[test]
fn query_lifetime_is_inferred_without_restricting_other_lifetimes() {
    let parsed: MultipleLifetimes<'_, 'static, 'static> =
        MultipleLifetimes::from_query("borrowed=value").expect("unrelated lifetimes are accepted");
    assert_eq!(parsed.borrowed, "value");

    let encoded = MultiLifetimeEncoder {
        first: "one",
        second: "two",
    };
    assert_eq!(
        encoded.to_query_string().expect("multiple lifetimes encode"),
        "first=one&second=two"
    );
}

#[test]
fn generated_generic_names_do_not_collide_with_user_parameters() {
    let parsed: GeneratedNameCollision<'static, GenericNested<u32>> =
        GeneratedNameCollision::from_query("nested=7").expect("generated names are unique");
    assert_eq!(parsed.nested.nested, 7);

    let defaulted: DefaultedGeneric = DefaultedGeneric::from_query("nested=8").expect("trailing defaults remain valid");
    assert_eq!(defaulted.nested.nested, 8);

    let higher_ranked: HigherRankedLifetime<for<'a> fn(&'a str)> =
        HigherRankedLifetime::from_query("value=ok").expect("higher-ranked lifetime names remain hygienic");
    assert_eq!(higher_ranked.value, "ok");

    let dependent: GenericDefaultBounds<'static, 4> =
        GenericDefaultBounds::from_query("value=bounded").expect("const and lifetime Default bounds are generated");
    assert_eq!(dependent.value, "bounded");
}

#[test]
fn flattened_strictness_is_order_independent_and_enforced_only_at_the_boundary() {
    let strict_first = StrictThenRight::from_query("right=7").expect("later flattened child claims key");
    assert_eq!(strict_first.right.right, Some(7));
    let strict_last = RightThenStrict::from_query("right=7").expect("earlier flattened child claims key");
    assert_eq!(strict_last.right.right, Some(7));

    assert_eq!(
        StrictOuter::from_query("unknown=7")
            .expect_err("outer strictness rejects a truly unknown key")
            .kind(),
        ErrorKind::UnknownParameter
    );
    assert_eq!(
        StrictThenRight::from_query("unknown=7")
            .expect_err("flattened strictness propagates to the outer boundary")
            .kind(),
        ErrorKind::UnknownParameter
    );
}

#[test]
fn overlapping_direct_and_flattened_fields_are_ambiguous() {
    let error = AmbiguousFlatten::from_query("shared=7").expect_err("two flattened children claim the key");
    assert_eq!(error.kind(), ErrorKind::AmbiguousParameter);
    assert_eq!(error.parameter(), None);

    assert_eq!(
        AmbiguousFlatten::from_query("shared=not-a-number")
            .expect_err("ambiguity is detected before either claimant parses")
            .kind(),
        ErrorKind::AmbiguousParameter
    );
    assert_eq!(
        DirectPrecedence::from_query("shared=7")
            .expect_err("a direct/flatten collision must not make the flattened field impossible")
            .kind(),
        ErrorKind::AmbiguousParameter
    );
}

#[test]
fn type_classification_uses_standard_paths_without_stealing_custom_or_generic_names() {
    let parsed = SpellingQuery::<u32>::from_query(
        "generic=1&custom_string=2&custom_option=3&custom_vec=4&standard_string=five&standard_option=6&standard_vec=7&standard_vec=8",
    )
    .expect("all spelling-sensitive field types parse");
    assert_eq!(
        parsed,
        SpellingQuery {
            generic: 1,
            custom_string: custom_names::String(2),
            custom_option: custom_names::Option(3),
            custom_vec: custom_names::Vec(4),
            standard_string: "five".to_owned(),
            standard_option: Some(6),
            standard_vec: vec![7, 8],
        }
    );
    assert_eq!(
        parsed.to_query_string().expect("all spelling-sensitive field types encode"),
        "generic=1&custom_string=2&custom_option=3&custom_vec=4&standard_string=five&standard_option=6&standard_vec=7&standard_vec=8"
    );
}

#[test]
fn generated_identifiers_and_copied_self_predicates_are_hygienic() {
    let parsed = State::<u32>::from_query("value=9").expect("local state and writer names do not collide");
    assert_eq!(parsed, State { value: 9 });
    assert_eq!(parsed.to_query_string().expect("method generic is unique"), "value=9");
}

#[test]
fn defaults_cover_containers_and_skipped_generic_cow() {
    assert_eq!(
        DefaultsOnContainers::from_query("").expect("container defaults match their normal missing behavior"),
        DefaultsOnContainers {
            optional: None,
            repeated: Vec::new(),
        }
    );

    let parsed = SkippedGenericCow::<str>::from_query("value=ok").expect("the skipped Cow receives its required Default bound");
    assert_eq!(parsed.value, "ok");
    assert!(parsed.skipped.is_empty());
}

#[test]
fn parses_borrowed_optional_repeated_and_default_fields() {
    let parsed = SearchQuery::from_query("q=rust&page=2&tag=fast&tags=safe").expect("valid query");
    assert_eq!(
        parsed,
        SearchQuery {
            q: Cow::Borrowed("rust"),
            page: Some(2),
            tag: vec![Cow::Borrowed("fast"), Cow::Borrowed("safe")],
            order: String::new(),
        }
    );
}

#[test]
fn applies_form_decoding_only_when_needed() {
    let parsed = SearchQuery::from_query("q=rust+language&tag=a%2Fb").expect("valid query");
    assert_eq!(parsed.q, "rust language");
    assert!(matches!(parsed.q, Cow::Owned(_)));
    assert_eq!(parsed.tag, ["a/b"]);
    assert!(matches!(parsed.tag[0], Cow::Owned(_)));

    let parsed = SearchQuery::from_query("q=%E2%9C%93").expect("valid non-ASCII escape");
    assert_eq!(parsed.q, "✓");
    assert!(matches!(parsed.q, Cow::Owned(_)));
}

#[test]
fn borrowed_fields_reject_values_that_require_decoding() {
    let error = Flattened::from_query("q=rust+lang&page=1").expect_err("borrowed value cannot own decoded data");
    assert_eq!(error.parameter(), Some("q"));
    assert_eq!(error.kind(), ErrorKind::BorrowRequired);
}

#[test]
fn reports_missing_duplicate_unknown_and_invalid_values() {
    let missing = SearchQuery::from_query("page=2").expect_err("q is required");
    assert_eq!(missing.parameter(), Some("q"));
    assert_eq!(missing.kind(), ErrorKind::Missing);

    let duplicate = SearchQuery::from_query("q=rust&q=again").expect_err("q is scalar");
    assert_eq!(duplicate.parameter(), Some("q"));
    assert_eq!(duplicate.pair_offset(), Some(7));
    assert_eq!(duplicate.kind(), ErrorKind::Duplicate);

    let unknown = SearchQuery::from_query("q=rust&extra=1").expect_err("unknown fields are denied");
    assert_eq!(unknown.parameter(), None);
    assert_eq!(unknown.pair_offset(), Some(7));
    assert_eq!(unknown.kind(), ErrorKind::UnknownParameter);

    let invalid = SearchQuery::from_query("q=rust&page=nope").expect_err("page must be numeric");
    assert_eq!(invalid.parameter(), Some("page"));
    assert_eq!(invalid.kind(), ErrorKind::InvalidValue);
}

#[test]
fn rejects_malformed_escapes_and_invalid_utf8() {
    assert_eq!(
        SearchQuery::from_query("q=%").expect_err("truncated escape").kind(),
        ErrorKind::InvalidEncoding
    );
    assert_eq!(
        SearchQuery::from_query("q=%A").expect_err("truncated escape").kind(),
        ErrorKind::InvalidEncoding
    );
    assert_eq!(
        SearchQuery::from_query("q=%AG").expect_err("invalid low nibble").kind(),
        ErrorKind::InvalidEncoding
    );
    assert_eq!(
        SearchQuery::from_query("q=%GG").expect_err("non-hex escape").kind(),
        ErrorKind::InvalidEncoding
    );
    assert_eq!(
        SearchQuery::from_query("q=%FF").expect_err("invalid UTF-8").kind(),
        ErrorKind::InvalidUtf8
    );
}

#[test]
fn flattening_preserves_single_pass_field_semantics() {
    let parsed = Flattened::from_query("q=rust&page=3").expect("valid flattened query");
    assert_eq!(
        parsed,
        Flattened {
            q: "rust",
            paging: Paging { page: 3, per_page: 0 },
        }
    );
    assert_eq!(
        parsed.to_query_string().expect("query production succeeds"),
        "q=rust&page=3&per_page=0"
    );
}

#[test]
fn supports_canonical_names_aliases_and_rename_all() {
    let parsed = Renamed::from_query("userName=ferris&limit=10").expect("alias parses");
    assert_eq!(
        parsed,
        Renamed {
            user_name: "ferris",
            maximum: 10,
        }
    );
    assert_eq!(
        parsed.to_query_string().expect("query production succeeds"),
        "userName=ferris&max=10"
    );
}

#[test]
fn produces_canonical_form_encoding_and_repeated_pairs() {
    let query = SearchQuery {
        q: Cow::Borrowed("rust language+"),
        page: None,
        tag: vec![Cow::Borrowed("a/b"), Cow::Borrowed("")],
        order: "name~asc".to_owned(),
    };
    assert_eq!(
        query.to_query_string().expect("query production succeeds"),
        "q=rust+language%2B&tag=a%2Fb&tag=&order=name%7Easc"
    );
}

#[test]
fn resource_limits_apply_to_parsing_and_production() {
    let input_length_limits = QueryLimits {
        max_query_length: 5,
        ..QueryLimits::UNLIMITED
    };
    assert_eq!(
        SearchQuery::from_query_with("q=rust", input_length_limits)
            .expect_err("input length limit")
            .kind(),
        ErrorKind::QueryTooLong
    );

    let parse_limits = QueryLimits {
        max_pairs: 1,
        ..QueryLimits::UNLIMITED
    };
    assert_eq!(
        SearchQuery::from_query_with("q=rust&page=2", parse_limits)
            .expect_err("pair limit")
            .kind(),
        ErrorKind::TooManyPairs
    );

    let decoded_limits = QueryLimits {
        max_decoded_length: 4,
        ..QueryLimits::UNLIMITED
    };
    assert_eq!(
        SearchQuery::from_query_with("q=rust", decoded_limits)
            .expect_err("decoded length limit")
            .kind(),
        ErrorKind::DecodedTooLong
    );

    let repeated_limits = QueryLimits {
        max_repeated_values: 1,
        ..QueryLimits::UNLIMITED
    };
    assert_eq!(
        SearchQuery::from_query_with("q=rust&tag=a&tag=b", repeated_limits)
            .expect_err("repeated value limit")
            .kind(),
        ErrorKind::TooManyValues
    );

    let query = Renamed {
        user_name: "ferris",
        maximum: 10,
    };
    let mut output = String::new();
    let write_limits = QueryLimits {
        max_encoded_length: 4,
        ..QueryLimits::UNLIMITED
    };
    let error = query.write_query_with(&mut output, write_limits).expect_err("encoded output limit");
    assert_eq!(error.kind(), ErrorKind::TooLong);
    assert_eq!(
        query
            .to_query_string_with(write_limits)
            .expect_err("owned encoding applies explicit limits")
            .kind(),
        ErrorKind::TooLong
    );
}

#[test]
fn empty_and_key_only_values_have_empty_string_semantics() {
    #[derive(Debug, PartialEq, Eq, routerama::query::FromQuery)]
    struct Empty<'q> {
        a: &'q str,
        b: &'q str,
    }

    assert_eq!(
        Empty::from_query("a&b=").expect("key-only and explicit empty values parse"),
        Empty { a: "", b: "" }
    );
}

#[test]
fn ignores_unknown_fields_by_default() {
    assert_eq!(
        Paging::from_query("ignored=x&page=2").expect("unknown field is ignored"),
        Paging { page: 2, per_page: 0 }
    );
}

#[test]
fn wide_queries_decode_escapes_and_round_trip_canonically() {
    #[derive(Debug, PartialEq, Eq, routerama::query::FromQuery, routerama::query::ToQuery)]
    struct Wide<'q> {
        #[query(rename = "search term")]
        value: Cow<'q, str>,
    }

    let input = "search+term=abcdefghijklmnopqrstuvwxyz%2F0123456789";
    let parsed = Wide::from_query(input).expect("wide escaped query parses");
    assert_eq!(parsed.value, "abcdefghijklmnopqrstuvwxyz/0123456789");
    let produced = parsed.to_query_string().expect("query production succeeds");
    assert_eq!(produced, "search+term=abcdefghijklmnopqrstuvwxyz%2F0123456789");
    assert_eq!(Wide::from_query(&produced).expect("produced query parses"), parsed);
}

#[test]
fn parses_and_produces_every_specialized_scalar() {
    let input = concat!(
        "unsigned8=1&unsigned16=2&unsigned32=3&unsigned64=4&unsigned128=5&pointer_unsigned=6&",
        "signed8=-1&signed16=-2&signed32=-3&signed64=-4&signed128=-5&pointer_signed=-6&enabled=false"
    );
    let parsed = NumericQuery::from_query(input).expect("numeric query parses");
    assert_eq!(
        parsed,
        NumericQuery {
            unsigned8: 1,
            unsigned16: 2,
            unsigned32: 3,
            unsigned64: 4,
            unsigned128: 5,
            pointer_unsigned: 6,
            signed8: -1,
            signed16: -2,
            signed32: -3,
            signed64: -4,
            signed128: -5,
            pointer_signed: -6,
            enabled: false,
        }
    );
    assert_eq!(parsed.to_query_string().expect("numeric query writes"), input);
}

#[test]
fn custom_display_values_escape_and_propagate_failures() {
    let escaped = DisplayQuery {
        value: DisplayValue("a b/~"),
    };
    assert_eq!(escaped.to_query_string().expect("display value writes"), "value=a+b%2F%7E");

    let format_error = FailingQuery { value: FailingDisplay }
        .to_query_string()
        .expect_err("display error propagates");
    assert_eq!(format_error.parameter(), Some("value"));
    assert_eq!(format_error.kind(), ErrorKind::Format);
    assert!(format_error.to_string().contains("field formatting failed"));

    let mut rejecting = RejectingWriter;
    let output_error = escaped.write_query(&mut rejecting).expect_err("writer error propagates");
    assert_eq!(output_error.parameter(), Some("value"));
    assert_eq!(output_error.kind(), ErrorKind::Output);

    let mut output = String::new();
    let limit_error = escaped
        .write_query_with(
            &mut output,
            QueryLimits {
                max_encoded_length: 7,
                ..QueryLimits::UNLIMITED
            },
        )
        .expect_err("display adapter propagates limits");
    assert_eq!(limit_error.parameter(), Some("value"));
    assert_eq!(limit_error.kind(), ErrorKind::TooLong);
}

#[test]
fn owned_strings_and_error_messages_cover_public_diagnostics() {
    #[derive(Debug, PartialEq, Eq, routerama::query::FromQuery)]
    struct Owned {
        value: String,
    }

    assert_eq!(
        Owned::from_query("value=owned+text").expect("owned value parses"),
        Owned {
            value: "owned text".to_owned(),
        }
    );

    let error = Owned::from_query("").expect_err("required value is missing");
    assert!(error.to_string().contains("required parameter is missing"));
    assert_eq!(error.parameter(), Some("value"));
    assert_eq!(error.pair_offset(), Some(0));
    assert!(Error::unknown(2).to_string().contains("parameter is not recognized by the schema"));

    assert_eq!(
        Owned::from_query("&&value=x&&").expect("empty pairs are ignored"),
        Owned { value: "x".to_owned() }
    );
}
