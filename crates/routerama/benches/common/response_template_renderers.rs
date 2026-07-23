// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use bytes::Bytes;
use http_body::{Frame, SizeHint};
use routerama::response::__template::{html_text, json_number, json_string, slot_len, write_slot};
use routerama::response::{Body, html_body_template, json_body_template};

const FULLY_STATIC: &str = "status=ready\nservice=routerama\n";
const NUMERIC_ID: u64 = 4_294_967_311;
const NUMERIC_PREFIX: &[u8] = br#"{"id":"#;
const NUMERIC_SUFFIX: &[u8] = br#","active":true}"#;
const NUMERIC_JSON: &str = r#"{"id":4294967311,"active":true}"#;
const ESCAPED_MESSAGE: &str = "quote: \"routerama\"\nline";
const ESCAPED_PREFIX: &[u8] = br#"{"message":"#;
const ESCAPED_SUFFIX: &[u8] = b"}";
const ESCAPED_JSON: &str = r#"{"message":"quote: \"routerama\"\nline"}"#;
const MEDIUM_PREFIX: &str = concat!(
    "<html><head><title>Routerama</title></head><body>",
    "<nav>home | routes | diagnostics</nav><main><h1>Hello, "
);
const MEDIUM_NAME: &str = "Ada";
const MEDIUM_SUFFIX: &str = concat!(
    "</h1><p>This medium shell contains fixed navigation, headings, and ",
    "descriptive text around one small dynamic insertion.</p></main>",
    "<footer>served by Routerama</footer></body></html>"
);
const MEDIUM_EXPECTED: &str = concat!(
    "<html><head><title>Routerama</title></head><body>",
    "<nav>home | routes | diagnostics</nav><main><h1>Hello, ",
    "Ada",
    "</h1><p>This medium shell contains fixed navigation, headings, and ",
    "descriptive text around one small dynamic insertion.</p></main>",
    "<footer>served by Routerama</footer></body></html>"
);

#[derive(serde::Serialize)]
struct EscapedPayload<'a> {
    message: &'a str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Representation {
    ExistingContiguous,
    ExactContiguous,
    Segmented,
}

impl Representation {
    const ALL: [Self; 3] = [Self::ExistingContiguous, Self::ExactContiguous, Self::Segmented];

    const fn name(self) -> &'static str {
        match self {
            Self::ExistingContiguous => "existing_contiguous",
            Self::ExactContiguous => "exact_contiguous",
            Self::Segmented => "segmented",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BodyScenario {
    FullyStatic,
    NumericJson,
    EscapedJson,
    MediumTextShell,
}

impl BodyScenario {
    const ALL: [Self; 4] = [
        Self::FullyStatic,
        Self::NumericJson,
        Self::EscapedJson,
        Self::MediumTextShell,
    ];

    const fn name(self) -> &'static str {
        match self {
            Self::FullyStatic => "fully_static",
            Self::NumericJson => "numeric_json",
            Self::EscapedJson => "escaped_json",
            Self::MediumTextShell => "medium_text_shell",
        }
    }

    const fn expected(self) -> &'static [u8] {
        match self {
            Self::FullyStatic => FULLY_STATIC.as_bytes(),
            Self::NumericJson => NUMERIC_JSON.as_bytes(),
            Self::EscapedJson => ESCAPED_JSON.as_bytes(),
            Self::MediumTextShell => MEDIUM_EXPECTED.as_bytes(),
        }
    }

    fn static_spans(self) -> StaticSpans {
        match self {
            Self::FullyStatic => StaticSpans::one(FULLY_STATIC.as_bytes()),
            Self::NumericJson => StaticSpans::two(NUMERIC_PREFIX, NUMERIC_SUFFIX),
            Self::EscapedJson => StaticSpans::two(ESCAPED_PREFIX, ESCAPED_SUFFIX),
            Self::MediumTextShell => StaticSpans::two(MEDIUM_PREFIX.as_bytes(), MEDIUM_SUFFIX.as_bytes()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct StaticSpan {
    address: usize,
    length: usize,
}

impl StaticSpan {
    fn of(bytes: &'static [u8]) -> Self {
        Self {
            address: bytes.as_ptr() as usize,
            length: bytes.len(),
        }
    }

    const fn end(self) -> usize {
        self.address + self.length
    }

    fn overlap(self, address: usize, length: usize) -> usize {
        let start = self.address.max(address);
        let end = self.end().min(address.saturating_add(length));
        end.saturating_sub(start)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct StaticSpans {
    spans: [Option<StaticSpan>; 2],
}

impl StaticSpans {
    fn one(bytes: &'static [u8]) -> Self {
        Self {
            spans: [Some(StaticSpan::of(bytes)), None],
        }
    }

    fn two(first: &'static [u8], second: &'static [u8]) -> Self {
        Self {
            spans: [Some(StaticSpan::of(first)), Some(StaticSpan::of(second))],
        }
    }

    const fn total_length(self) -> usize {
        let first = match self.spans[0] {
            Some(span) => span.length,
            None => 0,
        };
        let second = match self.spans[1] {
            Some(span) => span.length,
            None => 0,
        };
        first + second
    }

    fn overlap(self, address: usize, length: usize) -> usize {
        self.spans
            .into_iter()
            .flatten()
            .map(|span| span.overlap(address, length))
            .sum()
    }
}

#[derive(Clone, Copy)]
struct JsonNumber(u64);

#[derive(Clone, Copy)]
struct JsonString<'a>(&'a str);

#[derive(Clone, Copy)]
struct HtmlText<'a>(&'a str);

fn existing_contiguous_body(scenario: BodyScenario) -> Body {
    match scenario {
        BodyScenario::FullyStatic => Body::from(FULLY_STATIC),
        BodyScenario::NumericJson => Body::from(format!(r#"{{"id":{NUMERIC_ID},"active":true}}"#)),
        BodyScenario::EscapedJson => Body::from(
            serde_json::to_string(&EscapedPayload {
                message: ESCAPED_MESSAGE,
            })
            .expect("serializing the benchmark payload succeeds"),
        ),
        BodyScenario::MediumTextShell => Body::from(format!("{MEDIUM_PREFIX}{MEDIUM_NAME}{MEDIUM_SUFFIX}")),
    }
}

fn exact_contiguous_body(scenario: BodyScenario) -> Body {
    match scenario {
        BodyScenario::FullyStatic => html_body_template!("status=ready\n", "service=routerama\n"),
        BodyScenario::NumericJson => exact_numeric_json(JsonNumber(NUMERIC_ID)),
        BodyScenario::EscapedJson => exact_json_string(JsonString(ESCAPED_MESSAGE)),
        BodyScenario::MediumTextShell => exact_html_text(HtmlText(MEDIUM_NAME)),
    }
}

fn exact_numeric_json(value: JsonNumber) -> Body {
    json_body_template!(
        id = number(value.0);
        r#"{"id":"#, id, r#","active":true}"#
    )
}

fn exact_json_string(value: JsonString<'_>) -> Body {
    json_body_template!(
        message = string(value.0);
        r#"{"message":"#, message, "}"
    )
}

fn exact_html_text(value: HtmlText<'_>) -> Body {
    html_body_template!(
        name = text(value.0);
        "<html><head><title>Routerama</title></head><body>",
        "<nav>home | routes | diagnostics</nav><main><h1>Hello, ",
        name,
        "</h1><p>This medium shell contains fixed navigation, headings, and ",
        "descriptive text around one small dynamic insertion.</p></main>",
        "<footer>served by Routerama</footer></body></html>"
    )
}

#[derive(Clone, Debug)]
struct SegmentedBody {
    prefix: &'static [u8],
    dynamic: Option<Bytes>,
    suffix: &'static [u8],
    state: u8,
    remaining: usize,
}

impl SegmentedBody {
    fn new(prefix: &'static [u8], dynamic: Option<Bytes>, suffix: &'static [u8]) -> Self {
        let dynamic_length = dynamic.as_ref().map_or(0, Bytes::len);
        Self {
            prefix,
            dynamic,
            suffix,
            state: 0,
            remaining: prefix
                .len()
                .checked_add(dynamic_length)
                .and_then(|length| length.checked_add(suffix.len()))
                .expect("generated response template lengths must fit in usize"),
        }
    }

    fn static_body(bytes: &'static [u8]) -> Self {
        Self::new(bytes, None, b"")
    }
}

impl http_body::Body for SegmentedBody {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(mut self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        loop {
            let bytes = match self.state {
                0 => Bytes::from_static(self.prefix),
                1 => self.dynamic.take().unwrap_or_default(),
                2 => Bytes::from_static(self.suffix),
                _ => return Poll::Ready(None),
            };
            self.state += 1;
            if bytes.is_empty() {
                continue;
            }
            self.remaining = self
                .remaining
                .checked_sub(bytes.len())
                .expect("remaining length includes every generated response-template frame");
            return Poll::Ready(Some(Ok(Frame::data(bytes))));
        }
    }

    fn is_end_stream(&self) -> bool {
        self.remaining == 0
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::with_exact(self.remaining as u64)
    }
}

fn segmented_body(scenario: BodyScenario) -> SegmentedBody {
    match scenario {
        BodyScenario::FullyStatic => SegmentedBody::static_body(FULLY_STATIC.as_bytes()),
        BodyScenario::NumericJson => segmented_numeric_json(JsonNumber(NUMERIC_ID)),
        BodyScenario::EscapedJson => segmented_json_string(JsonString(ESCAPED_MESSAGE)),
        BodyScenario::MediumTextShell => segmented_html_text(HtmlText(MEDIUM_NAME)),
    }
}

fn segmented_numeric_json(value: JsonNumber) -> SegmentedBody {
    let slot = json_number(value.0);
    let mut dynamic = Vec::with_capacity(slot_len(&slot));
    write_slot(&mut dynamic, &slot);
    SegmentedBody::new(NUMERIC_PREFIX, Some(Bytes::from(dynamic)), NUMERIC_SUFFIX)
}

fn segmented_json_string(value: JsonString<'_>) -> SegmentedBody {
    let slot = json_string(value.0);
    let mut dynamic = Vec::with_capacity(slot_len(&slot));
    write_slot(&mut dynamic, &slot);
    SegmentedBody::new(ESCAPED_PREFIX, Some(Bytes::from(dynamic)), ESCAPED_SUFFIX)
}

fn segmented_html_text(value: HtmlText<'_>) -> SegmentedBody {
    let slot = html_text(value.0);
    let mut dynamic = Vec::with_capacity(slot_len(&slot));
    write_slot(&mut dynamic, &slot);
    SegmentedBody::new(
        MEDIUM_PREFIX.as_bytes(),
        Some(Bytes::from(dynamic)),
        MEDIUM_SUFFIX.as_bytes(),
    )
}

fn expected_frame_lengths(representation: Representation, scenario: BodyScenario) -> ([usize; 3], usize) {
    if representation != Representation::Segmented {
        return ([scenario.expected().len(), 0, 0], 1);
    }

    match scenario {
        BodyScenario::FullyStatic => ([FULLY_STATIC.len(), 0, 0], 1),
        BodyScenario::NumericJson => (
            [
                NUMERIC_PREFIX.len(),
                slot_len(&json_number(NUMERIC_ID)),
                NUMERIC_SUFFIX.len(),
            ],
            3,
        ),
        BodyScenario::EscapedJson => (
            [
                ESCAPED_PREFIX.len(),
                slot_len(&json_string(ESCAPED_MESSAGE)),
                ESCAPED_SUFFIX.len(),
            ],
            3,
        ),
        BodyScenario::MediumTextShell => (
            [
                MEDIUM_PREFIX.len(),
                slot_len(&html_text(MEDIUM_NAME)),
                MEDIUM_SUFFIX.len(),
            ],
            3,
        ),
    }
}

fn expected_copied_static_bytes(representation: Representation, scenario: BodyScenario) -> usize {
    match representation {
        Representation::ExistingContiguous => scenario.static_spans().total_length(),
        Representation::ExactContiguous => {
            if matches!(scenario, BodyScenario::FullyStatic) {
                0
            } else {
                scenario.static_spans().total_length()
            }
        }
        Representation::Segmented => 0,
    }
}
