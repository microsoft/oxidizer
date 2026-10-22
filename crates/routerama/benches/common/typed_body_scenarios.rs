// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

// Shared JsonBody/FormBody construction scenarios. Both wrappers serialize
// eagerly in their constructor (`JsonBody::new`, `FormBody::new`), so the
// encode cost is directly attributable to the constructor call. Payload
// construction is excluded; measured work starts at the constructor call and
// ends after observing the encoded length and content checksum.

use routerama::query::{FromQuery, ToQuery};
use routerama::route::form::FormBody;
use routerama::route::json::JsonBody;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToQuery, FromQuery)]
struct Flat {
    id: u32,
    active: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToQuery, FromQuery)]
struct Escaped {
    name: String,
    note: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToQuery, FromQuery)]
struct Large {
    id: u32,
    name: String,
    description: String,
    tags: Vec<u32>,
    active: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scenario {
    Flat,
    Escaped,
    Large,
}

impl Scenario {
    const ALL: [Self; 3] = [Self::Flat, Self::Escaped, Self::Large];

    const fn name(self) -> &'static str {
        match self {
            Self::Flat => "flat",
            Self::Escaped => "escaped",
            Self::Large => "large",
        }
    }
}

fn flat_payload() -> Flat {
    Flat { id: 42, active: true }
}

fn escaped_payload() -> Escaped {
    Escaped {
        name: "quote\" backslash\\ newline\n tab\t \u{2713}".to_owned(),
        note: "reserved & chars = need % encoding".to_owned(),
    }
}

fn large_payload() -> Large {
    Large {
        id: 1_000_000,
        name: "large-payload".to_owned(),
        description: "x".repeat(200),
        tags: (0..32).collect(),
        active: true,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Observation {
    length: usize,
    hash: u64,
}

fn fingerprint(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325_u64, |mut hash, &byte| {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        hash
    })
}

impl Observation {
    fn of(bytes: &[u8]) -> Self {
        Self {
            length: bytes.len(),
            hash: fingerprint(bytes),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PreparedFlat(Flat);
#[derive(Clone, Debug, PartialEq, Eq)]
struct PreparedEscaped(Escaped);
#[derive(Clone, Debug, PartialEq, Eq)]
struct PreparedLarge(Large);

fn prepare_flat() -> PreparedFlat {
    PreparedFlat(flat_payload())
}

fn prepare_escaped() -> PreparedEscaped {
    PreparedEscaped(escaped_payload())
}

fn prepare_large() -> PreparedLarge {
    PreparedLarge(large_payload())
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "the benchmark input is taken by value to match Gungraun's #[bench::run(...)] calling convention"
)]
fn run_json_flat_prepared(prepared: PreparedFlat) -> Observation {
    let body = JsonBody::new(&prepared.0).expect("the flat payload always serializes as JSON");
    Observation::of(body.as_bytes())
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "the benchmark input is taken by value to match Gungraun's #[bench::run(...)] calling convention"
)]
fn run_json_escaped_prepared(prepared: PreparedEscaped) -> Observation {
    let body = JsonBody::new(&prepared.0).expect("the escaped payload always serializes as JSON");
    Observation::of(body.as_bytes())
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "the benchmark input is taken by value to match Gungraun's #[bench::run(...)] calling convention"
)]
fn run_json_large_prepared(prepared: PreparedLarge) -> Observation {
    let body = JsonBody::new(&prepared.0).expect("the large payload always serializes as JSON");
    Observation::of(body.as_bytes())
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "the benchmark input is taken by value to match Gungraun's #[bench::run(...)] calling convention"
)]
fn run_form_flat_prepared(prepared: PreparedFlat) -> Observation {
    let body = FormBody::new(&prepared.0).expect("the flat payload always encodes as a form");
    Observation::of(body.as_str().as_bytes())
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "the benchmark input is taken by value to match Gungraun's #[bench::run(...)] calling convention"
)]
fn run_form_escaped_prepared(prepared: PreparedEscaped) -> Observation {
    let body = FormBody::new(&prepared.0).expect("the escaped payload always encodes as a form");
    Observation::of(body.as_str().as_bytes())
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "the benchmark input is taken by value to match Gungraun's #[bench::run(...)] calling convention"
)]
fn run_form_large_prepared(prepared: PreparedLarge) -> Observation {
    let body = FormBody::new(&prepared.0).expect("the large payload always encodes as a form");
    Observation::of(body.as_str().as_bytes())
}

fn assert_equivalent() {
    let flat = flat_payload();
    let json_flat = JsonBody::new(&flat).expect("the flat payload always serializes as JSON");
    let decoded: Flat = serde_json::from_slice(json_flat.as_bytes()).expect("JsonBody output round-trips through serde_json");
    assert_eq!(decoded, flat, "JsonBody/flat did not round-trip to the source payload");
    let form_flat = FormBody::new(&flat).expect("the flat payload always encodes as a form");
    let decoded: Flat = Flat::from_query(form_flat.as_str()).expect("FormBody output round-trips through the query decoder");
    assert_eq!(decoded, flat, "FormBody/flat did not round-trip to the source payload");

    let escaped = escaped_payload();
    let json_escaped = JsonBody::new(&escaped).expect("the escaped payload always serializes as JSON");
    let decoded: Escaped = serde_json::from_slice(json_escaped.as_bytes()).expect("JsonBody output round-trips through serde_json");
    assert_eq!(decoded, escaped, "JsonBody/escaped did not round-trip to the source payload");
    let form_escaped = FormBody::new(&escaped).expect("the escaped payload always encodes as a form");
    let decoded: Escaped = Escaped::from_query(form_escaped.as_str()).expect("FormBody output round-trips through the query decoder");
    assert_eq!(decoded, escaped, "FormBody/escaped did not round-trip to the source payload");

    let large = large_payload();
    let json_large = JsonBody::new(&large).expect("the large payload always serializes as JSON");
    let decoded: Large = serde_json::from_slice(json_large.as_bytes()).expect("JsonBody output round-trips through serde_json");
    assert_eq!(decoded, large, "JsonBody/large did not round-trip to the source payload");
    let form_large = FormBody::new(&large).expect("the large payload always encodes as a form");
    let decoded: Large = Large::from_query(form_large.as_str()).expect("FormBody output round-trips through the query decoder");
    assert_eq!(decoded, large, "FormBody/large did not round-trip to the source payload");
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AllocationStats {
    allocations: u64,
    bytes: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AllocationComparison {
    json: AllocationStats,
    form: AllocationStats,
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

fn allocation_diagnostics() -> [(Scenario, AllocationComparison); 3] {
    Scenario::ALL.map(|scenario| {
        let comparison = match scenario {
            Scenario::Flat => AllocationComparison {
                json: measure_allocations(|| run_json_flat_prepared(std::hint::black_box(prepare_flat()))),
                form: measure_allocations(|| run_form_flat_prepared(std::hint::black_box(prepare_flat()))),
            },
            Scenario::Escaped => AllocationComparison {
                json: measure_allocations(|| run_json_escaped_prepared(std::hint::black_box(prepare_escaped()))),
                form: measure_allocations(|| run_form_escaped_prepared(std::hint::black_box(prepare_escaped()))),
            },
            Scenario::Large => AllocationComparison {
                json: measure_allocations(|| run_json_large_prepared(std::hint::black_box(prepare_large()))),
                form: measure_allocations(|| run_form_large_prepared(std::hint::black_box(prepare_large()))),
            },
        };
        (scenario, comparison)
    })
}
