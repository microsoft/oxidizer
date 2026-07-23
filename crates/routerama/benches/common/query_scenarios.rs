// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::borrow::Cow;
use std::hint::black_box;

use routerama::query::{FromQuery, ToQuery};
use serde::{Deserialize, Serialize};

const COMMON: &str = "q=rust&page=2&exact=true";
const ESCAPED: &str = "q=rust+language%2Fweb&page=2&exact=true";
const REPEATED: &str = "q=rust&tag=fast&tag=safe&tag=zero+alloc";
const LONG_VALUE: &str = concat!(
    "payload=",
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
);
const LONG_ESCAPED_VALUE: &str = concat!(
    "a /", "a /", "a /", "a /", "a /", "a /", "a /", "a /", "a /", "a /", "a /", "a /", "a /", "a /", "a /", "a /",
    "a /", "a /", "a /", "a /", "a /", "a /", "a /", "a /", "a /", "a /", "a /", "a /", "a /", "a /", "a /", "a /",
);
const LONG_ESCAPED_OUTPUT: &str = concat!(
    "value=",
    "a+%2F",
    "a+%2F",
    "a+%2F",
    "a+%2F",
    "a+%2F",
    "a+%2F",
    "a+%2F",
    "a+%2F",
    "a+%2F",
    "a+%2F",
    "a+%2F",
    "a+%2F",
    "a+%2F",
    "a+%2F",
    "a+%2F",
    "a+%2F",
    "a+%2F",
    "a+%2F",
    "a+%2F",
    "a+%2F",
    "a+%2F",
    "a+%2F",
    "a+%2F",
    "a+%2F",
    "a+%2F",
    "a+%2F",
    "a+%2F",
    "a+%2F",
    "a+%2F",
    "a+%2F",
    "a+%2F",
    "a+%2F",
);

#[derive(Debug, routerama::query::FromQuery, routerama::query::ToQuery)]
struct DirectCommon<'q> {
    q: Cow<'q, str>,
    page: u32,
    exact: bool,
}

#[derive(Debug, Deserialize, Serialize)]
struct SerdeCommon<'q> {
    #[serde(borrow)]
    q: Cow<'q, str>,
    page: u32,
    exact: bool,
}

#[derive(Debug, routerama::query::FromQuery, routerama::query::ToQuery)]
struct DirectRepeated {
    q: String,
    tag: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct SerdeRepeated {
    q: String,
    tag: Vec<String>,
}

#[derive(Debug, routerama::query::FromQuery, routerama::query::ToQuery)]
struct DirectLong<'q> {
    payload: &'q str,
}

#[derive(Debug, Deserialize, Serialize)]
struct SerdeLong<'q> {
    payload: &'q str,
}

#[derive(Debug, routerama::query::ToQuery)]
struct PrimitiveOutput {
    count: u64,
    enabled: bool,
}

#[derive(Debug, routerama::query::ToQuery)]
struct OptionalOutput<'q> {
    q: Option<&'q str>,
    page: Option<u32>,
}

#[derive(Debug, routerama::query::ToQuery)]
struct RepeatedOutput<'q> {
    tag: Vec<&'q str>,
}

#[derive(Debug, routerama::query::ToQuery)]
struct EscapedOutput<'q> {
    value: &'q str,
}

#[derive(Debug, routerama::query::ToQuery)]
#[expect(
    clippy::empty_structs_with_brackets,
    reason = "query derives deliberately require a named-field struct, including the empty-schema control"
)]
struct EmptyOutput {}

fn direct_parse_common() {
    black_box(DirectCommon::from_query(black_box(COMMON)).expect("valid query"));
}

fn serde_urlencoded_parse_common() {
    black_box(serde_urlencoded::from_str::<SerdeCommon<'_>>(black_box(COMMON)).expect("valid query"));
}

fn serde_html_form_parse_common() {
    black_box(serde_html_form::from_str::<SerdeCommon<'_>>(black_box(COMMON)).expect("valid query"));
}

fn direct_parse_escaped() {
    black_box(DirectCommon::from_query(black_box(ESCAPED)).expect("valid query"));
}

fn serde_urlencoded_parse_escaped() {
    black_box(serde_urlencoded::from_str::<SerdeCommon<'_>>(black_box(ESCAPED)).expect("valid query"));
}

fn serde_html_form_parse_escaped() {
    black_box(serde_html_form::from_str::<SerdeCommon<'_>>(black_box(ESCAPED)).expect("valid query"));
}

fn direct_parse_repeated() {
    black_box(DirectRepeated::from_query(black_box(REPEATED)).expect("valid query"));
}

fn serde_html_form_parse_repeated() {
    black_box(serde_html_form::from_str::<SerdeRepeated>(black_box(REPEATED)).expect("valid query"));
}

fn direct_parse_long() {
    black_box(DirectLong::from_query(black_box(LONG_VALUE)).expect("valid query"));
}

fn serde_urlencoded_parse_long() {
    black_box(serde_urlencoded::from_str::<SerdeLong<'_>>(black_box(LONG_VALUE)).expect("valid query"));
}

fn serde_html_form_parse_long() {
    black_box(serde_html_form::from_str::<SerdeLong<'_>>(black_box(LONG_VALUE)).expect("valid query"));
}

fn direct_produce_common(query: &DirectCommon<'_>, output: &mut String) {
    output.clear();
    black_box(query)
        .write_query(black_box(output))
        .expect("query production succeeds");
    black_box(output);
}

fn direct_produce_common_allocating(query: &DirectCommon<'_>) {
    black_box(black_box(query).to_query_string().expect("query production succeeds"));
}

fn serde_urlencoded_produce_common(query: &SerdeCommon<'_>) {
    black_box(serde_urlencoded::to_string(black_box(query)).expect("query production succeeds"));
}

fn serde_html_form_produce_common(query: &SerdeCommon<'_>) {
    black_box(serde_html_form::to_string(black_box(query)).expect("query production succeeds"));
}

fn serde_html_form_produce_common_reserved(query: &SerdeCommon<'_>, output: &mut String) {
    output.clear();
    serde_html_form::push_to_string(black_box(output), black_box(query)).expect("query production succeeds");
    black_box(output);
}

fn direct_common_value() -> DirectCommon<'static> {
    DirectCommon {
        q: Cow::Borrowed("rust"),
        page: 2,
        exact: true,
    }
}

fn serde_common_value() -> SerdeCommon<'static> {
    SerdeCommon {
        q: Cow::Borrowed("rust"),
        page: 2,
        exact: true,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OutputShape {
    Primitive,
    OptionalPresent,
    OptionalAbsent,
    Repeated,
    EscapedShort,
    EscapedLong,
    Empty,
}

impl OutputShape {
    const ALL: [Self; 7] = [
        Self::Primitive,
        Self::OptionalPresent,
        Self::OptionalAbsent,
        Self::Repeated,
        Self::EscapedShort,
        Self::EscapedLong,
        Self::Empty,
    ];

    const fn name(self) -> &'static str {
        match self {
            Self::Primitive => "primitive",
            Self::OptionalPresent => "optional_present",
            Self::OptionalAbsent => "optional_absent",
            Self::Repeated => "repeated",
            Self::EscapedShort => "escaped_short",
            Self::EscapedLong => "escaped_long",
            Self::Empty => "empty",
        }
    }

    const fn expected(self) -> &'static str {
        match self {
            Self::Primitive => "count=18446744073709551615&enabled=true",
            Self::OptionalPresent => "q=rust&page=2",
            Self::OptionalAbsent | Self::Empty => "",
            Self::Repeated => "tag=fast&tag=safe&tag=zero+alloc",
            Self::EscapedShort => "value=a+b%2Fc",
            Self::EscapedLong => LONG_ESCAPED_OUTPUT,
        }
    }
}

enum PreparedOutput {
    Primitive(PrimitiveOutput),
    OptionalPresent(OptionalOutput<'static>),
    OptionalAbsent(OptionalOutput<'static>),
    Repeated(RepeatedOutput<'static>),
    EscapedShort(EscapedOutput<'static>),
    EscapedLong(EscapedOutput<'static>),
    Empty(EmptyOutput),
}

fn prepare_output_shape(shape: OutputShape) -> PreparedOutput {
    match shape {
        OutputShape::Primitive => PreparedOutput::Primitive(PrimitiveOutput {
            count: u64::MAX,
            enabled: true,
        }),
        OutputShape::OptionalPresent => PreparedOutput::OptionalPresent(OptionalOutput {
            q: Some("rust"),
            page: Some(2),
        }),
        OutputShape::OptionalAbsent => PreparedOutput::OptionalAbsent(OptionalOutput { q: None, page: None }),
        OutputShape::Repeated => PreparedOutput::Repeated(RepeatedOutput {
            tag: vec!["fast", "safe", "zero alloc"],
        }),
        OutputShape::EscapedShort => PreparedOutput::EscapedShort(EscapedOutput { value: "a b/c" }),
        OutputShape::EscapedLong => PreparedOutput::EscapedLong(EscapedOutput {
            value: LONG_ESCAPED_VALUE,
        }),
        OutputShape::Empty => PreparedOutput::Empty(EmptyOutput {}),
    }
}

fn run_output_shape(prepared: &PreparedOutput) -> String {
    match prepared {
        PreparedOutput::Primitive(value) => value.to_query_string(),
        PreparedOutput::OptionalPresent(value) | PreparedOutput::OptionalAbsent(value) => value.to_query_string(),
        PreparedOutput::Repeated(value) => value.to_query_string(),
        PreparedOutput::EscapedShort(value) | PreparedOutput::EscapedLong(value) => value.to_query_string(),
        PreparedOutput::Empty(value) => value.to_query_string(),
    }
    .expect("query production succeeds")
}

fn write_output_shape(prepared: &PreparedOutput, output: &mut String) {
    match prepared {
        PreparedOutput::Primitive(value) => value.write_query(output),
        PreparedOutput::OptionalPresent(value) | PreparedOutput::OptionalAbsent(value) => value.write_query(output),
        PreparedOutput::Repeated(value) => value.write_query(output),
        PreparedOutput::EscapedShort(value) | PreparedOutput::EscapedLong(value) => value.write_query(output),
        PreparedOutput::Empty(value) => value.write_query(output),
    }
    .expect("query production succeeds");
}

fn assert_output_shapes() {
    for shape in OutputShape::ALL {
        let prepared = prepare_output_shape(shape);
        let owned = run_output_shape(&prepared);
        let mut streamed = String::new();
        write_output_shape(&prepared, &mut streamed);
        assert_eq!(owned, shape.expected(), "query output shape {} changed", shape.name());
        assert_eq!(streamed, owned, "owned and streamed query output differ for {}", shape.name());
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct OutputDiagnostic {
    shape: OutputShape,
    allocations: u64,
    bytes: u64,
    length: usize,
    capacity: usize,
}

fn output_diagnostics() -> [OutputDiagnostic; 7] {
    OutputShape::ALL.map(|shape| {
        let prepared = prepare_output_shape(shape);
        let session = alloc_tracker::Session::new().no_stdout().no_file();
        let operation = session.operation("produce");
        let output = {
            let _span = operation.measure_thread().iterations(1);
            std::hint::black_box(run_output_shape(&prepared))
        };
        let report = session.to_report();
        let (_, operation) = report
            .operations()
            .find(|(name, _)| *name == "produce")
            .expect("the query-output allocation operation is recorded");
        OutputDiagnostic {
            shape,
            allocations: operation.total_allocations_count(),
            bytes: operation.total_bytes_allocated(),
            length: output.len(),
            capacity: output.capacity(),
        }
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ReservedOutputDiagnostic {
    allocations: u64,
    bytes: u64,
    length: usize,
    capacity: usize,
}

fn reserved_output_diagnostic() -> ReservedOutputDiagnostic {
    let query = direct_common_value();
    let mut output = String::with_capacity(COMMON.len());
    let session = alloc_tracker::Session::new().no_stdout().no_file();
    let operation = session.operation("produce_reserved");
    {
        let _span = operation.measure_thread().iterations(1);
        direct_produce_common(&query, &mut output);
    }
    let report = session.to_report();
    let (_, operation) = report
        .operations()
        .find(|(name, _)| *name == "produce_reserved")
        .expect("the reserved query-output allocation operation is recorded");
    assert_eq!(output, COMMON);
    ReservedOutputDiagnostic {
        allocations: operation.total_allocations_count(),
        bytes: operation.total_bytes_allocated(),
        length: output.len(),
        capacity: output.capacity(),
    }
}
