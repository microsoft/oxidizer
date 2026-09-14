// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Shared HTTP and custom-source decode operations for parser-shape benchmarks.

use std::hint::black_box;

use http::{HeaderMap, HeaderName, HeaderValue};
use http_headers::source::{FieldLines, FieldSource};
use http_headers::{DecodeError, DecodeErrorKind, DecodeMode, Field, FieldName, FieldValueRef};

#[derive(Clone, Copy, Debug)]
pub(crate) enum Expected {
    Valid,
    Absent,
    Error(DecodeErrorKind),
}

#[derive(Clone, Copy)]
pub(crate) struct Expectations {
    pub(crate) http_owned: Expected,
    pub(crate) http_borrowed: Expected,
    pub(crate) raw_owned: Expected,
    pub(crate) raw_borrowed: Expected,
}

impl From<Expected> for Expectations {
    fn from(expected: Expected) -> Self {
        Self {
            http_owned: expected,
            http_borrowed: expected,
            raw_owned: expected,
            raw_borrowed: expected,
        }
    }
}

impl Expectations {
    fn for_operation(self, raw: bool, owned: bool) -> Expected {
        match (raw, owned) {
            (false, false) => self.http_borrowed,
            (false, true) => self.http_owned,
            (true, false) => self.raw_borrowed,
            (true, true) => self.raw_owned,
        }
    }
}

pub(crate) struct Fixture {
    http: HeaderMap,
    raw: RawSource,
}

struct RawSource {
    name: &'static FieldName,
    values: Vec<FieldValueRef<'static>>,
}

impl FieldSource for RawSource {
    #[inline]
    fn lines(&self, name: &'static FieldName) -> Option<FieldLines<'_>> {
        (name == self.name).then(|| FieldLines::from_borrowed(name, &self.values)).flatten()
    }
}

impl Fixture {
    pub(crate) fn new<F: Field>(values: &'static [&'static str], mode: DecodeMode, expected: impl Into<Expectations>) -> Self {
        let expected = expected.into();
        let name = F::name();
        let http_name = HeaderName::from_static(name.as_str());
        let mut http = HeaderMap::with_capacity(1);
        for value in values {
            let mut value = HeaderValue::from_bytes(value.as_bytes()).expect("fixture is a valid HTTP field value");
            value.set_sensitive(matches!(name.as_str(), "authorization" | "set-cookie"));
            http.append(&http_name, value);
        }
        let fixture = Self {
            http,
            raw: RawSource {
                name,
                values: values.iter().map(|value| FieldValueRef::new(value.as_bytes())).collect(),
            },
        };
        check(F::view_with(&fixture.http, mode), expected.http_borrowed, "HTTP borrowed");
        check(F::owned_with(&fixture.http, mode), expected.http_owned, "HTTP owned");
        check(F::view_with(&fixture.raw, mode), expected.raw_borrowed, "raw borrowed");
        check(F::owned_with(&fixture.raw, mode), expected.raw_owned, "raw owned");
        fixture
    }
}

#[expect(clippy::panic, reason = "invalid benchmark fixtures must fail setup")]
fn check<T>(result: Result<Option<T>, DecodeError>, expected: Expected, operation: &str) {
    match (result, expected) {
        (Ok(Some(_)), Expected::Valid) | (Ok(None), Expected::Absent) => {}
        (Err(error), Expected::Error(kind)) => assert_eq!(error.kind(), kind, "{operation}: {error:?}"),
        (Err(error), _) => panic!("{operation}: fixture unexpectedly failed: {error:?}"),
        (Ok(_), _) => panic!("{operation}: fixture did not produce {expected:?}"),
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Case {
    fixture: &'static Fixture,
    operation: fn(&Fixture),
}

impl Case {
    #[inline]
    pub(crate) fn run(self) {
        (self.operation)(black_box(self.fixture));
    }
}

const fn mode<const RELAXED: bool>() -> DecodeMode {
    if RELAXED { DecodeMode::Relaxed } else { DecodeMode::Strict }
}

#[expect(clippy::inline_always, reason = "result consumption belongs inside each measured decode operation")]
#[expect(clippy::panic, reason = "a benchmark fixture must retain its expected outcome")]
#[inline(always)]
fn consume<T, const OUTCOME: u8>(result: Result<Option<T>, DecodeError>) {
    match OUTCOME {
        0 => drop(black_box(result.expect("fixture must decode").expect("fixture must be present"))),
        1 => assert!(black_box(result.expect("absent fixture must decode")).is_none()),
        _ => match result {
            Err(error) => {
                let _error = black_box(error);
            }
            Ok(_) => panic!("fixture must fail"),
        },
    }
}

#[inline(never)]
fn http_borrowed<F: Field, const RELAXED: bool, const OUTCOME: u8>(fixture: &Fixture) {
    consume::<_, OUTCOME>(F::view_with(black_box(&fixture.http), mode::<RELAXED>()));
}

#[inline(never)]
fn http_owned<F: Field, const RELAXED: bool, const OUTCOME: u8>(fixture: &Fixture) {
    consume::<_, OUTCOME>(F::owned_with(black_box(&fixture.http), mode::<RELAXED>()));
}

#[inline(never)]
fn raw_borrowed<F: Field, const RELAXED: bool, const OUTCOME: u8>(fixture: &Fixture) {
    consume::<_, OUTCOME>(F::view_with(black_box(&fixture.raw), mode::<RELAXED>()));
}

#[inline(never)]
fn raw_owned<F: Field, const RELAXED: bool, const OUTCOME: u8>(fixture: &Fixture) {
    consume::<_, OUTCOME>(F::owned_with(black_box(&fixture.raw), mode::<RELAXED>()));
}

fn operation<F: Field, const RELAXED: bool, const OUTCOME: u8>(raw: bool, owned: bool) -> fn(&Fixture) {
    match (raw, owned) {
        (false, false) => http_borrowed::<F, RELAXED, OUTCOME>,
        (false, true) => http_owned::<F, RELAXED, OUTCOME>,
        (true, false) => raw_borrowed::<F, RELAXED, OUTCOME>,
        (true, true) => raw_owned::<F, RELAXED, OUTCOME>,
    }
}

pub(crate) fn case<F: Field>(
    fixture: &'static Fixture,
    mode: DecodeMode,
    expected: impl Into<Expectations>,
    raw: bool,
    owned: bool,
) -> Case {
    let operation = match (mode, expected.into().for_operation(raw, owned)) {
        (DecodeMode::Strict, Expected::Valid) => operation::<F, false, 0>(raw, owned),
        (DecodeMode::Strict, Expected::Absent) => operation::<F, false, 1>(raw, owned),
        (DecodeMode::Strict, Expected::Error(_)) => operation::<F, false, 2>(raw, owned),
        (DecodeMode::Relaxed, Expected::Valid) => operation::<F, true, 0>(raw, owned),
        (DecodeMode::Relaxed, Expected::Absent) => operation::<F, true, 1>(raw, owned),
        (DecodeMode::Relaxed, Expected::Error(_)) => operation::<F, true, 2>(raw, owned),
    };
    Case { fixture, operation }
}

macro_rules! define_shapes {
    ($group:literal; $(($id:ident, $header:ty, $values:expr, $mode:ident, $expected:expr)),+ $(,)?) => {
        paste::paste! {
            $(
                fn [<$id _fixture>]() -> &'static $crate::shapes::Fixture {
                    static FIXTURE: std::sync::OnceLock<$crate::shapes::Fixture> = std::sync::OnceLock::new();
                    FIXTURE.get_or_init(|| $crate::shapes::Fixture::new::<$header>(
                        $values,
                        http_headers::DecodeMode::$mode,
                        $expected,
                    ))
                }

                fn [<$id _http_owned>]() -> $crate::shapes::Case {
                    $crate::shapes::case::<$header>(
                        [<$id _fixture>](), http_headers::DecodeMode::$mode, $expected, false, true,
                    )
                }
                fn [<$id _http_borrowed>]() -> $crate::shapes::Case {
                    $crate::shapes::case::<$header>(
                        [<$id _fixture>](), http_headers::DecodeMode::$mode, $expected, false, false,
                    )
                }
                fn [<$id _raw_owned>]() -> $crate::shapes::Case {
                    $crate::shapes::case::<$header>(
                        [<$id _fixture>](), http_headers::DecodeMode::$mode, $expected, true, true,
                    )
                }
                fn [<$id _raw_borrowed>]() -> $crate::shapes::Case {
                    $crate::shapes::case::<$header>(
                        [<$id _fixture>](), http_headers::DecodeMode::$mode, $expected, true, false,
                    )
                }
            )+

            #[metabench::benchmark(HTTP_OWNED, $group, "http_owned")]
            $(#[bench::$id(setup = [<$id _http_owned>])])+
            fn http_owned(case: $crate::shapes::Case) { case.run(); }

            #[metabench::benchmark(HTTP_BORROWED, $group, "http_borrowed")]
            $(#[bench::$id(setup = [<$id _http_borrowed>])])+
            fn http_borrowed(case: $crate::shapes::Case) { case.run(); }

            #[metabench::benchmark(RAW_OWNED, $group, "raw_owned")]
            $(#[bench::$id(setup = [<$id _raw_owned>])])+
            fn raw_owned(case: $crate::shapes::Case) { case.run(); }

            #[metabench::benchmark(RAW_BORROWED, $group, "raw_borrowed")]
            $(#[bench::$id(setup = [<$id _raw_borrowed>])])+
            fn raw_borrowed(case: $crate::shapes::Case) { case.run(); }

            fn criterion_benchmarks(criterion: &mut criterion::Criterion) {
                let mut group = criterion.benchmark_group($group);
                $(
                    group.bench_function(
                        criterion::BenchmarkId::new(HTTP_OWNED.benchmark_name(), stringify!($id)),
                        |b| b.iter_batched([<$id _http_owned>], http_owned, criterion::BatchSize::SmallInput),
                    );
                    group.bench_function(
                        criterion::BenchmarkId::new(HTTP_BORROWED.benchmark_name(), stringify!($id)),
                        |b| b.iter_batched([<$id _http_borrowed>], http_borrowed, criterion::BatchSize::SmallInput),
                    );
                    group.bench_function(
                        criterion::BenchmarkId::new(RAW_OWNED.benchmark_name(), stringify!($id)),
                        |b| b.iter_batched([<$id _raw_owned>], raw_owned, criterion::BatchSize::SmallInput),
                    );
                    group.bench_function(
                        criterion::BenchmarkId::new(RAW_BORROWED.benchmark_name(), stringify!($id)),
                        |b| b.iter_batched([<$id _raw_borrowed>], raw_borrowed, criterion::BatchSize::SmallInput),
                    );
                )+
                group.finish();
            }

            metabench::main!(
                criterion = {
                    factory = criterion::Criterion::default,
                    benchmarks = criterion_benchmarks,
                    unit = "ns",
                },
                benchmarks = [HTTP_OWNED, HTTP_BORROWED, RAW_OWNED, RAW_BORROWED],
            );
        }
    };
}

pub(crate) use define_shapes;
