// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

// Shared response-head construction scenarios across three representations:
// "reserved" (runtime-validated names/values inserted one at a time after
// `HeaderMap::reserve`), "static_plan" (a batched `HeaderMap::extend` over
// compile-time-validated name/value constants), and "generated" (the plan a
// `#[route(..., headers(insert(...)))]` expansion emits: individual
// `HeaderMap::insert` calls over the same compile-time-validated constants).
// Scenario construction is excluded; measured work starts at header
// insertion and ends after observing the header count and content checksum.

use http::header::{HeaderName, HeaderValue};
use routerama::response::{Body, Response};

const HEADER_FIELDS: [(&str, &str); 16] = [
    ("x-template-00", "value-00"),
    ("x-template-01", "value-01"),
    ("x-template-02", "value-02"),
    ("x-template-03", "value-03"),
    ("x-template-04", "value-04"),
    ("x-template-05", "value-05"),
    ("x-template-06", "value-06"),
    ("x-template-07", "value-07"),
    ("x-template-08", "value-08"),
    ("x-template-09", "value-09"),
    ("x-template-10", "value-10"),
    ("x-template-11", "value-11"),
    ("x-template-12", "value-12"),
    ("x-template-13", "value-13"),
    ("x-template-14", "value-14"),
    ("x-template-15", "value-15"),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scenario {
    Headers0,
    Headers1,
    Headers4,
    Headers16,
}

impl Scenario {
    const ALL: [Self; 4] = [Self::Headers0, Self::Headers1, Self::Headers4, Self::Headers16];

    const fn name(self) -> &'static str {
        match self {
            Self::Headers0 => "headers_0",
            Self::Headers1 => "headers_1",
            Self::Headers4 => "headers_4",
            Self::Headers16 => "headers_16",
        }
    }

    const fn count(self) -> usize {
        match self {
            Self::Headers0 => 0,
            Self::Headers1 => 1,
            Self::Headers4 => 4,
            Self::Headers16 => 16,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Observation {
    length: usize,
    hash: u64,
}

fn checksum<'a>(fields: impl IntoIterator<Item = (&'a str, &'a str)>) -> u64 {
    fields.into_iter().fold(0xcbf2_9ce4_8422_2325_u64, |mut hash, (name, value)| {
        for byte in name.bytes().chain(value.bytes()) {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash
    })
}

fn expected(scenario: Scenario) -> Observation {
    let count = scenario.count();
    Observation {
        length: count,
        hash: checksum(HEADER_FIELDS[..count].iter().copied()),
    }
}

fn observe(response: &Response<Body>) -> Observation {
    Observation {
        length: response.headers().len(),
        hash: checksum(response.headers().iter().map(|(name, value)| {
            (
                name.as_str(),
                value.to_str().expect("the fixture header values are ASCII"),
            )
        })),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PreparedScenario {
    scenario: Scenario,
}

fn prepare(scenario: Scenario) -> PreparedScenario {
    PreparedScenario { scenario }
}

macro_rules! extend_headers {
    ($headers:expr, [$(($name:literal, $value:literal)),* $(,)?]) => {{
        $headers.extend([
            $(
                (
                    Some(const { HeaderName::from_static($name) }),
                    const { HeaderValue::from_static($value) },
                ),
            )*
        ]);
    }};
}

macro_rules! insert_generated {
    ($headers:expr, [$(($name:literal, $value:literal)),* $(,)?]) => {{
        $(
            _ = $headers.insert(
                const { HeaderName::from_static($name) },
                const { HeaderValue::from_static($value) },
            );
        )*
    }};
}

fn build_reserved(scenario: Scenario) -> Response<Body> {
    let mut response = Response::new(Body::empty());
    let count = scenario.count();
    response.headers_mut().reserve(count);
    for &(name, value) in &HEADER_FIELDS[..count] {
        response.headers_mut().insert(HeaderName::from_static(name), HeaderValue::from_static(value));
    }
    response
}

fn build_static_plan(scenario: Scenario) -> Response<Body> {
    let mut response = Response::new(Body::empty());
    match scenario {
        Scenario::Headers0 => {}
        Scenario::Headers1 => extend_headers!(response.headers_mut(), [("x-template-00", "value-00")]),
        Scenario::Headers4 => extend_headers!(
            response.headers_mut(),
            [
                ("x-template-00", "value-00"),
                ("x-template-01", "value-01"),
                ("x-template-02", "value-02"),
                ("x-template-03", "value-03"),
            ]
        ),
        Scenario::Headers16 => extend_headers!(
            response.headers_mut(),
            [
                ("x-template-00", "value-00"),
                ("x-template-01", "value-01"),
                ("x-template-02", "value-02"),
                ("x-template-03", "value-03"),
                ("x-template-04", "value-04"),
                ("x-template-05", "value-05"),
                ("x-template-06", "value-06"),
                ("x-template-07", "value-07"),
                ("x-template-08", "value-08"),
                ("x-template-09", "value-09"),
                ("x-template-10", "value-10"),
                ("x-template-11", "value-11"),
                ("x-template-12", "value-12"),
                ("x-template-13", "value-13"),
                ("x-template-14", "value-14"),
                ("x-template-15", "value-15"),
            ]
        ),
    }
    response
}

fn build_generated(scenario: Scenario) -> Response<Body> {
    let mut response = Response::new(Body::empty());
    match scenario {
        Scenario::Headers0 => {}
        Scenario::Headers1 => insert_generated!(response.headers_mut(), [("x-template-00", "value-00")]),
        Scenario::Headers4 => insert_generated!(
            response.headers_mut(),
            [
                ("x-template-00", "value-00"),
                ("x-template-01", "value-01"),
                ("x-template-02", "value-02"),
                ("x-template-03", "value-03"),
            ]
        ),
        Scenario::Headers16 => insert_generated!(
            response.headers_mut(),
            [
                ("x-template-00", "value-00"),
                ("x-template-01", "value-01"),
                ("x-template-02", "value-02"),
                ("x-template-03", "value-03"),
                ("x-template-04", "value-04"),
                ("x-template-05", "value-05"),
                ("x-template-06", "value-06"),
                ("x-template-07", "value-07"),
                ("x-template-08", "value-08"),
                ("x-template-09", "value-09"),
                ("x-template-10", "value-10"),
                ("x-template-11", "value-11"),
                ("x-template-12", "value-12"),
                ("x-template-13", "value-13"),
                ("x-template-14", "value-14"),
                ("x-template-15", "value-15"),
            ]
        ),
    }
    response
}

fn run_reserved_prepared(prepared: PreparedScenario) -> Observation {
    observe(&build_reserved(prepared.scenario))
}

fn run_static_plan_prepared(prepared: PreparedScenario) -> Observation {
    observe(&build_static_plan(prepared.scenario))
}

fn run_generated_prepared(prepared: PreparedScenario) -> Observation {
    observe(&build_generated(prepared.scenario))
}

fn assert_equivalent() {
    for scenario in Scenario::ALL {
        let expected = expected(scenario);
        assert_eq!(
            run_reserved_prepared(prepare(scenario)),
            expected,
            "reserved/{} changed its header count or content",
            scenario.name()
        );
        assert_eq!(
            run_static_plan_prepared(prepare(scenario)),
            expected,
            "static_plan/{} differs from reserved",
            scenario.name()
        );
        assert_eq!(
            run_generated_prepared(prepare(scenario)),
            expected,
            "generated/{} differs from reserved",
            scenario.name()
        );
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AllocationStats {
    allocations: u64,
    bytes: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AllocationComparison {
    reserved: AllocationStats,
    static_plan: AllocationStats,
    generated: AllocationStats,
}

fn measure_allocations(run: impl FnOnce() -> Observation) -> AllocationStats {
    let session = alloc_tracker::Session::new().no_stdout().no_file();
    let operation = session.operation("construction");
    {
        let _span = operation.measure_thread().iterations(1);
        std::hint::black_box(run());
    }
    let report = session.to_report();
    let (_, operation) = report
        .operations()
        .find(|(name, _)| *name == "construction")
        .expect("the construction allocation operation is recorded");
    AllocationStats {
        allocations: operation.total_allocations_count(),
        bytes: operation.total_bytes_allocated(),
    }
}

fn allocation_diagnostics() -> [(Scenario, AllocationComparison); 4] {
    Scenario::ALL.map(|scenario| {
        let reserved = std::hint::black_box(prepare(scenario));
        let reserved = measure_allocations(|| run_reserved_prepared(reserved));
        let static_plan = std::hint::black_box(prepare(scenario));
        let static_plan = measure_allocations(|| run_static_plan_prepared(static_plan));
        let generated = std::hint::black_box(prepare(scenario));
        let generated = measure_allocations(|| run_generated_prepared(generated));
        (
            scenario,
            AllocationComparison {
                reserved,
                static_plan,
                generated,
            },
        )
    })
}
