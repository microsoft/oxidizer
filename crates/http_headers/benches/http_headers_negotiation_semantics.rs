// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Decode, repeated semantic reads and selection against fixed offer sets.
//!
//! The policy is explicit: more specific preferences override less specific
//! ones, the first duplicate wins, zero excludes an offer, and offer order
//! breaks equal-quality ties. This benchmark is not a negotiation policy API.
//! Raw cases include the caller-side parsing required by the original API.

#![expect(clippy::unwrap_used, reason = "benchmark fixtures are independently validated during setup")]

use std::cmp::Ordering;
use std::hint::black_box;

use http_headers::headers::{
    Accept, AcceptEncoding, AcceptEncodingEntry, AcceptEntry, AcceptLanguage, AcceptLanguageEntry, ContentCodingKind, MediaRangeKind,
    QualityView,
};
use http_headers::sink::{EncodedValues, FieldSink, InsertError};
use http_headers::source::{FieldLines, FieldSource};
use http_headers::{DecodeMode, Field, FieldName};

#[derive(Clone, Copy)]
enum Family {
    Media,
    Coding,
    Language,
}

#[derive(Clone, Copy)]
struct Case {
    family: Family,
    reads: usize,
}

struct Source {
    name: &'static FieldName,
    bytes: &'static [u8],
}

impl FieldSource for Source {
    fn lines(&self, name: &'static FieldName) -> Option<FieldLines<'_>> {
        (name == self.name).then(|| FieldLines::single(name, self.bytes))
    }
}

impl Case {
    fn source(self) -> Source {
        match self.family {
            Family::Media => Source {
                name: &FieldName::Accept,
                bytes: b"Text/HTML;level=\"1\";q=0.7;flag, application/json;q=0.8, text/*;q=0.9, */*;q=0.1, text/plain;q=0, application/json;q=0.8",
            },
            Family::Coding => Source {
                name: &FieldName::AcceptEncoding,
                bytes: b"BR;q=0, gzip;q=.8000000000000000000001, identity;q=0.8, *;q=0, x-private;q=1",
            },
            Family::Language => Source {
                name: &FieldName::AcceptLanguage,
                bytes: b"en;q=0.5, EN-us;q=0, fr;q=0.75, *;q=0.1, fr-CH;q=0.7500",
            },
        }
    }
}

#[derive(Clone, Copy)]
struct Candidate<Q> {
    quality: Q,
    specificity: usize,
    seen: bool,
}

impl<Q: Copy + Ord> Candidate<Q> {
    const fn new(zero: Q) -> Self {
        Self {
            quality: zero,
            specificity: 0,
            seen: false,
        }
    }

    fn consider(&mut self, quality: Q, specificity: usize) {
        if !self.seen || specificity > self.specificity {
            *self = Self {
                quality,
                specificity,
                seen: true,
            };
        }
    }
}

fn choose<Q: Copy + Ord>(candidates: [Candidate<Q>; 3], zero: Q) -> usize {
    let mut selected = 3;
    let mut quality = zero;
    for (index, candidate) in candidates.into_iter().enumerate() {
        if candidate.seen && candidate.quality > quality {
            selected = index;
            quality = candidate.quality;
        }
    }
    selected
}

const MEDIA: [(&str, &str); 3] = [("text", "html"), ("application", "json"), ("text", "plain")];
const CODINGS: [&str; 3] = ["br", "gzip", "identity"];
const LANGUAGES: [&str; 3] = ["en-US", "fr-CH", "de"];

fn typed_media<'a>(entries: impl Iterator<Item = AcceptEntry<'a>>) -> usize {
    let mut candidates = [Candidate::new(QualityView::ZERO); 3];
    for entry in entries {
        let range = entry.range();
        let mut parameter_count = 0;
        let mut parameters_match = true;
        for parameter in entry.parameters() {
            parameter_count += 1;
            parameters_match &= parameter.name().eq_ignore_ascii_case("level")
                && parameter
                    .value()
                    .is_some_and(|value| value.decoded_bytes().eq(b"1".iter().copied()));
        }
        for (index, (type_, subtype)) in MEDIA.into_iter().enumerate() {
            if parameter_count != 0 && (index != 0 || !parameters_match) {
                continue;
            }
            let specificity = match range.kind() {
                MediaRangeKind::Any => 0,
                MediaRangeKind::TypeWildcard if range.type_().eq_ignore_ascii_case(type_) => 1,
                MediaRangeKind::Exact if range.type_().eq_ignore_ascii_case(type_) && range.subtype().eq_ignore_ascii_case(subtype) => {
                    2 + parameter_count
                }
                _ => continue,
            };
            candidates[index].consider(entry.quality(), specificity);
        }
    }
    choose(candidates, QualityView::ZERO)
}

fn language_matches(range: &str, offered: &str) -> bool {
    offered.eq_ignore_ascii_case(range)
        || (offered.len() > range.len() && offered.as_bytes()[range.len()] == b'-' && offered[..range.len()].eq_ignore_ascii_case(range))
}

fn typed_coding<'a>(entries: impl Iterator<Item = AcceptEncodingEntry<'a>>) -> usize {
    let mut candidates = [Candidate::new(QualityView::ZERO); 3];
    for entry in entries {
        for (index, offer) in CODINGS.into_iter().enumerate() {
            let specificity = if entry.coding().kind() == ContentCodingKind::Wildcard {
                0
            } else if entry.coding().token().eq_ignore_ascii_case(offer) {
                1
            } else {
                continue;
            };
            candidates[index].consider(entry.quality(), specificity);
        }
    }
    choose(candidates, QualityView::ZERO)
}

fn typed_language<'a>(entries: impl Iterator<Item = AcceptLanguageEntry<'a>>) -> usize {
    let mut candidates = [Candidate::new(QualityView::ZERO); 3];
    for entry in entries {
        let range = entry.range();
        for (index, offer) in LANGUAGES.into_iter().enumerate() {
            let specificity = if range.is_wildcard() {
                0
            } else if language_matches(range.as_str(), offer) {
                1 + range.subtags().count()
            } else {
                continue;
            };
            candidates[index].consider(entry.quality(), specificity);
        }
    }
    choose(candidates, QualityView::ZERO)
}

fn typed(case: Case) -> usize {
    let source = case.source();
    let mut checksum = 0;
    match case.family {
        Family::Media => {
            let value = Accept::view_with(black_box(&source), DecodeMode::Relaxed).unwrap().unwrap();
            for _ in 0..case.reads {
                checksum += typed_media(black_box(&value).entries());
            }
            black_box(value.values().next().unwrap().as_bytes());
        }
        Family::Coding => {
            let value = AcceptEncoding::view_with(black_box(&source), DecodeMode::Relaxed).unwrap().unwrap();
            for _ in 0..case.reads {
                checksum += typed_coding(black_box(&value).entries());
            }
            black_box(value.values().next().unwrap().as_bytes());
        }
        Family::Language => {
            let value = AcceptLanguage::view_with(black_box(&source), DecodeMode::Relaxed).unwrap().unwrap();
            for _ in 0..case.reads {
                checksum += typed_language(black_box(&value).entries());
            }
            black_box(value.values().next().unwrap().as_bytes());
        }
    }
    checksum
}

#[derive(Clone, Copy, Eq)]
struct RawQuality<'a> {
    whole: bool,
    fraction: &'a [u8],
}

impl<'a> RawQuality<'a> {
    const ZERO: Self = Self {
        whole: false,
        fraction: b"",
    };
    const ONE: Self = Self {
        whole: true,
        fraction: b"",
    };

    fn parse(bytes: &'a [u8]) -> Self {
        let bytes = trim(bytes);
        let whole = bytes[0] == b'1';
        let fraction = bytes
            .iter()
            .position(|byte| *byte == b'.')
            .map_or(b"".as_slice(), |dot| &bytes[dot + 1..]);
        Self { whole, fraction }
    }
}

impl PartialEq for RawQuality<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Ord for RawQuality<'_> {
    fn cmp(&self, other: &Self) -> Ordering {
        let whole = self.whole.cmp(&other.whole);
        if whole != Ordering::Equal {
            return whole;
        }
        for index in 0..self.fraction.len().max(other.fraction.len()) {
            let left = self.fraction.get(index).copied().unwrap_or(b'0');
            let right = other.fraction.get(index).copied().unwrap_or(b'0');
            let order = left.cmp(&right);
            if order != Ordering::Equal {
                return order;
            }
        }
        Ordering::Equal
    }
}

impl PartialOrd for RawQuality<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn trim(bytes: &[u8]) -> &[u8] {
    bytes.trim_ascii()
}

fn parameters(bytes: &[u8]) -> impl Iterator<Item = &[u8]> {
    let mut remaining = Some(bytes);
    std::iter::from_fn(move || {
        let bytes = remaining?;
        let mut quoted = false;
        let mut escaped = false;
        for (index, byte) in bytes.iter().copied().enumerate() {
            if escaped {
                escaped = false;
            } else if quoted && byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                quoted = !quoted;
            } else if !quoted && byte == b';' {
                remaining = Some(&bytes[index + 1..]);
                return Some(trim(&bytes[..index]));
            }
        }
        remaining = None;
        Some(trim(bytes))
    })
}

fn split_parameter(bytes: &[u8]) -> (&[u8], Option<&[u8]>) {
    bytes
        .iter()
        .position(|byte| *byte == b'=')
        .map_or((bytes, None), |equals| (trim(&bytes[..equals]), Some(trim(&bytes[equals + 1..]))))
}

fn decoded_equals(bytes: &[u8], expected: &[u8]) -> bool {
    let quoted = bytes.first() == Some(&b'"');
    let bytes = if quoted { &bytes[1..bytes.len() - 1] } else { bytes };
    let mut bytes = bytes.iter().copied();
    std::iter::from_fn(move || {
        let byte = bytes.next()?;
        if quoted && byte == b'\\' { bytes.next() } else { Some(byte) }
    })
    .eq(expected.iter().copied())
}

fn raw_selection<'a>(family: Family, items: impl Iterator<Item = &'a [u8]>) -> usize {
    let mut candidates = [Candidate::new(RawQuality::ZERO); 3];
    for item in items {
        let mut segments = parameters(item);
        let head = segments.next().unwrap();
        let mut quality = RawQuality::ONE;
        let mut parameter_count = 0;
        let mut parameters_match = true;
        for parameter in segments {
            let (name, value) = split_parameter(parameter);
            if name.eq_ignore_ascii_case(b"q") {
                quality = RawQuality::parse(value.unwrap());
                break;
            }
            parameter_count += 1;
            parameters_match &= name.eq_ignore_ascii_case(b"level") && value.is_some_and(|value| decoded_equals(value, b"1"));
        }
        match family {
            Family::Media => {
                let slash = head.iter().position(|byte| *byte == b'/').unwrap();
                let (type_, subtype) = (&head[..slash], &head[slash + 1..]);
                for (index, (offered_type, offered_subtype)) in MEDIA.into_iter().enumerate() {
                    if parameter_count != 0 && (index != 0 || !parameters_match) {
                        continue;
                    }
                    let specificity = if type_ == b"*" {
                        0
                    } else if type_.eq_ignore_ascii_case(offered_type.as_bytes()) && subtype == b"*" {
                        1
                    } else if type_.eq_ignore_ascii_case(offered_type.as_bytes())
                        && subtype.eq_ignore_ascii_case(offered_subtype.as_bytes())
                    {
                        2 + parameter_count
                    } else {
                        continue;
                    };
                    candidates[index].consider(quality, specificity);
                }
            }
            Family::Coding => {
                for (index, offer) in CODINGS.into_iter().enumerate() {
                    let specificity = if head == b"*" {
                        0
                    } else if head.eq_ignore_ascii_case(offer.as_bytes()) {
                        1
                    } else {
                        continue;
                    };
                    candidates[index].consider(quality, specificity);
                }
            }
            Family::Language => {
                let range = std::str::from_utf8(head).unwrap();
                for (index, offer) in LANGUAGES.into_iter().enumerate() {
                    let specificity = if range == "*" {
                        0
                    } else if language_matches(range, offer) {
                        range.split('-').count()
                    } else {
                        continue;
                    };
                    candidates[index].consider(quality, specificity);
                }
            }
        }
    }
    choose(candidates, RawQuality::ZERO)
}

fn raw(case: Case) -> usize {
    let source = case.source();
    let mut checksum = 0;
    macro_rules! consume {
        ($header:ty) => {{
            let value = <$header>::view_with(black_box(&source), DecodeMode::Relaxed).unwrap().unwrap();
            for _ in 0..case.reads {
                checksum += raw_selection(case.family, black_box(&value).items());
            }
            black_box(value.values().next().unwrap().as_bytes());
        }};
    }
    match case.family {
        Family::Media => consume!(Accept),
        Family::Coding => consume!(AcceptEncoding),
        Family::Language => consume!(AcceptLanguage),
    }
    checksum
}

fn retained(case: Case) -> usize {
    let source = case.source();
    let mut checksum = 0;
    match case.family {
        Family::Media => {
            let value = Accept::view_with(black_box(&source), DecodeMode::Relaxed).unwrap().unwrap();
            for entry in value.entries() {
                for _ in 0..case.reads {
                    let entry = black_box(entry);
                    checksum += entry.range().type_().as_str().len() + usize::from(!entry.quality().is_zero());
                }
            }
        }
        Family::Coding => {
            let value = AcceptEncoding::view_with(black_box(&source), DecodeMode::Relaxed).unwrap().unwrap();
            for entry in value.entries() {
                for _ in 0..case.reads {
                    let entry = black_box(entry);
                    checksum += entry.coding().as_str().len() + usize::from(!entry.quality().is_zero());
                }
            }
        }
        Family::Language => {
            let value = AcceptLanguage::view_with(black_box(&source), DecodeMode::Relaxed).unwrap().unwrap();
            for entry in value.entries() {
                for _ in 0..case.reads {
                    let entry = black_box(entry);
                    checksum +=
                        entry.range().primary().map_or(0, |primary| primary.as_str().len()) + usize::from(!entry.quality().is_zero());
                }
            }
        }
    }
    checksum
}

struct Forward;

impl FieldSource for Forward {
    fn lines(&self, _name: &'static FieldName) -> Option<FieldLines<'_>> {
        None
    }
}

impl FieldSink for Forward {
    fn set_values(&mut self, name: &'static FieldName, values: EncodedValues) -> Result<(), InsertError> {
        black_box(name);
        for value in values {
            black_box(value);
        }
        Ok(())
    }

    fn append_values(&mut self, name: &'static FieldName, values: EncodedValues) -> Result<(), InsertError> {
        self.set_values(name, values)
    }

    fn remove_values(&mut self, name: &'static FieldName) {
        black_box(name);
    }
}

fn forward(case: Case, caller_parsed: bool) {
    let source = case.source();
    macro_rules! forward {
        ($header:ty, $select:ident) => {{
            let value = <$header>::owned_with(black_box(&source), DecodeMode::Relaxed).unwrap().unwrap();
            let selected = if caller_parsed {
                raw_selection(case.family, value.items())
            } else {
                $select(value.entries())
            };
            black_box(selected);
            <$header>::insert(&mut Forward, value).unwrap();
        }};
    }
    match case.family {
        Family::Media => forward!(Accept, typed_media),
        Family::Coding => forward!(AcceptEncoding, typed_coding),
        Family::Language => forward!(AcceptLanguage, typed_language),
    }
}

fn setup(family: Family, reads: usize) -> Case {
    let case = Case { family, reads };
    assert_eq!(typed(case), reads, "offer one is independently expected for every fixture");
    assert_eq!(raw(case), reads, "caller-side parsing must select the same offer");
    case
}

fn media_one() -> Case {
    setup(Family::Media, 1)
}
fn media_two() -> Case {
    setup(Family::Media, 2)
}
fn media_eight() -> Case {
    setup(Family::Media, 8)
}
fn coding_one() -> Case {
    setup(Family::Coding, 1)
}
fn coding_two() -> Case {
    setup(Family::Coding, 2)
}
fn coding_eight() -> Case {
    setup(Family::Coding, 8)
}
fn language_one() -> Case {
    setup(Family::Language, 1)
}
fn language_two() -> Case {
    setup(Family::Language, 2)
}
fn language_eight() -> Case {
    setup(Family::Language, 8)
}

#[metabench::benchmark(TYPED, "http_headers_negotiation_semantics/select", "typed")]
#[bench::media_one(setup = media_one)]
#[bench::media_two(setup = media_two)]
#[bench::media_eight(setup = media_eight)]
#[bench::coding_one(setup = coding_one)]
#[bench::coding_two(setup = coding_two)]
#[bench::coding_eight(setup = coding_eight)]
#[bench::language_one(setup = language_one)]
#[bench::language_two(setup = language_two)]
#[bench::language_eight(setup = language_eight)]
fn typed_selection(case: Case) {
    black_box(typed(case));
}

#[metabench::benchmark(RAW, "http_headers_negotiation_semantics/select", "caller_parsed")]
#[bench::media_one(setup = media_one)]
#[bench::media_two(setup = media_two)]
#[bench::media_eight(setup = media_eight)]
#[bench::coding_one(setup = coding_one)]
#[bench::coding_two(setup = coding_two)]
#[bench::coding_eight(setup = coding_eight)]
#[bench::language_one(setup = language_one)]
#[bench::language_two(setup = language_two)]
#[bench::language_eight(setup = language_eight)]
fn raw_selection_workload(case: Case) {
    black_box(raw(case));
}

#[metabench::benchmark(RETAINED, "http_headers_negotiation_semantics/retained", "scalar_reads")]
#[bench::media_eight(setup = media_eight)]
#[bench::coding_eight(setup = coding_eight)]
#[bench::language_eight(setup = language_eight)]
fn retained_reads(case: Case) {
    black_box(retained(case));
}

#[metabench::benchmark(FORWARD, "http_headers_negotiation_semantics/forward", "select_and_forward")]
#[bench::media_one(setup = media_one)]
#[bench::coding_one(setup = coding_one)]
#[bench::language_one(setup = language_one)]
fn selection_forward(case: Case) {
    forward(case, false);
}

#[metabench::benchmark(RAW_FORWARD, "http_headers_negotiation_semantics/forward", "caller_parsed_and_forward")]
#[bench::media_one(setup = media_one)]
#[bench::coding_one(setup = coding_one)]
#[bench::language_one(setup = language_one)]
fn raw_selection_forward(case: Case) {
    forward(case, true);
}

#[metabench::benchmark(DECODE, "http_headers_negotiation_semantics/decode", "borrowed")]
#[bench::media_one(setup = media_one)]
#[bench::coding_one(setup = coding_one)]
#[bench::language_one(setup = language_one)]
fn decode_only(case: Case) {
    let source = case.source();
    match case.family {
        Family::Media => {
            let _value = black_box(Accept::view_with(black_box(&source), DecodeMode::Relaxed).unwrap().unwrap());
        }
        Family::Coding => {
            let _value = black_box(AcceptEncoding::view_with(black_box(&source), DecodeMode::Relaxed).unwrap().unwrap());
        }
        Family::Language => {
            let _value = black_box(AcceptLanguage::view_with(black_box(&source), DecodeMode::Relaxed).unwrap().unwrap());
        }
    }
}

fn criterion_benchmarks(criterion: &mut criterion::Criterion) {
    type Setup = fn() -> Case;
    let cases: [(&str, Setup); 9] = [
        ("media_one", media_one),
        ("media_two", media_two),
        ("media_eight", media_eight),
        ("coding_one", coding_one),
        ("coding_two", coding_two),
        ("coding_eight", coding_eight),
        ("language_one", language_one),
        ("language_two", language_two),
        ("language_eight", language_eight),
    ];
    let mut group = criterion.benchmark_group("http_headers_negotiation_semantics/select");
    for (name, setup) in cases {
        group.bench_function(criterion::BenchmarkId::new(TYPED.benchmark_name(), name), |b| {
            b.iter_batched(setup, typed_selection, criterion::BatchSize::SmallInput);
        });
        group.bench_function(criterion::BenchmarkId::new(RAW.benchmark_name(), name), |b| {
            b.iter_batched(setup, raw_selection_workload, criterion::BatchSize::SmallInput);
        });
    }
    group.finish();
    let mut group = criterion.benchmark_group("http_headers_negotiation_semantics/retained");
    for (name, setup) in [cases[2], cases[5], cases[8]] {
        group.bench_function(criterion::BenchmarkId::new(RETAINED.benchmark_name(), name), |b| {
            b.iter_batched(setup, retained_reads, criterion::BatchSize::SmallInput);
        });
    }
    group.finish();
    let mut group = criterion.benchmark_group("http_headers_negotiation_semantics/forward");
    for (name, setup) in [cases[0], cases[3], cases[6]] {
        group.bench_function(criterion::BenchmarkId::new(FORWARD.benchmark_name(), name), |b| {
            b.iter_batched(setup, selection_forward, criterion::BatchSize::SmallInput);
        });
        group.bench_function(criterion::BenchmarkId::new(RAW_FORWARD.benchmark_name(), name), |b| {
            b.iter_batched(setup, raw_selection_forward, criterion::BatchSize::SmallInput);
        });
    }
    group.finish();
    let mut group = criterion.benchmark_group("http_headers_negotiation_semantics/decode");
    for (name, setup) in [cases[0], cases[3], cases[6]] {
        group.bench_function(criterion::BenchmarkId::new(DECODE.benchmark_name(), name), |b| {
            b.iter_batched(setup, decode_only, criterion::BatchSize::SmallInput);
        });
    }
    group.finish();
}

metabench::main!(
    criterion = {
        factory = criterion::Criterion::default,
        benchmarks = criterion_benchmarks,
        unit = "ns",
    },
    benchmarks = [TYPED, RAW, RETAINED, FORWARD, RAW_FORWARD, DECODE],
);
