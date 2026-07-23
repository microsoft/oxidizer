// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

// Routerama-only TextBody and Utf8Body extraction comparisons. Payload
// construction is excluded; measured work starts at FromRequestBody and ends
// after observing the extracted text or typed rejection.

use std::fmt;
use std::pin::{Pin, pin};
use std::task::{Context, Poll, Waker};

use bytes::Bytes;
use http::{Request, StatusCode};
use http_body::{Body as HttpBody, Frame, SizeHint};
use routerama::response::IntoResponse as _;
use routerama::route::{BodyRejection, FromRequestBody, TextBody, Utf8Body};

const BODY_LIMIT: usize = 64;
const SINGLE_TEXT: &str = "bounded UTF-8: \u{2713}";
const SPLIT_FIRST: &[u8] = b"split-";
const SPLIT_SECOND: &[u8] = b"body";
const AT_LIMIT: [u8; BODY_LIMIT] = [b'x'; BODY_LIMIT];
const OVER_LIMIT: [u8; BODY_LIMIT + 1] = [b'x'; BODY_LIMIT + 1];
const TRANSPORT_FAILURE: &str = "fixture body failed";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FixtureError(&'static str);

impl fmt::Display for FixtureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for FixtureError {}

#[derive(Debug)]
struct FixtureBody {
    first: Option<Bytes>,
    second: Option<Bytes>,
    failure: Option<FixtureError>,
}

impl FixtureBody {
    fn from_scenario(scenario: Scenario) -> Self {
        let (first, second, failure) = match scenario {
            Scenario::Empty => (None, None, None),
            Scenario::Single => (Some(Bytes::from_static(SINGLE_TEXT.as_bytes())), None, None),
            Scenario::Split => (Some(Bytes::from_static(SPLIT_FIRST)), Some(Bytes::from_static(SPLIT_SECOND)), None),
            Scenario::ExactLimit => (Some(Bytes::from_static(&AT_LIMIT)), None, None),
            Scenario::InvalidUtf8 => (Some(Bytes::from_static(b"text-\xff")), None, None),
            Scenario::Overflow => (Some(Bytes::from_static(&OVER_LIMIT)), None, None),
            Scenario::BodyError => (None, None, Some(FixtureError(TRANSPORT_FAILURE))),
        };
        Self { first, second, failure }
    }

    fn from_single(bytes: Bytes) -> Self {
        Self {
            first: Some(bytes),
            second: None,
            failure: None,
        }
    }

    fn remaining(&self) -> usize {
        self.first.as_ref().map_or(0, Bytes::len) + self.second.as_ref().map_or(0, Bytes::len)
    }
}

impl HttpBody for FixtureBody {
    type Data = Bytes;
    type Error = FixtureError;

    fn poll_frame(mut self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        if let Some(error) = self.failure.take() {
            return Poll::Ready(Some(Err(error)));
        }
        Poll::Ready(self.first.take().or_else(|| self.second.take()).map(|bytes| Ok(Frame::data(bytes))))
    }

    fn is_end_stream(&self) -> bool {
        self.first.is_none() && self.second.is_none() && self.failure.is_none()
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::with_exact(self.remaining() as u64)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scenario {
    Empty,
    Single,
    Split,
    ExactLimit,
    InvalidUtf8,
    Overflow,
    BodyError,
}

impl Scenario {
    const ALL: [Self; 7] = [
        Self::Empty,
        Self::Single,
        Self::Split,
        Self::ExactLimit,
        Self::InvalidUtf8,
        Self::Overflow,
        Self::BodyError,
    ];

    const fn name(self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::Single => "single",
            Self::Split => "split",
            Self::ExactLimit => "exact_limit",
            Self::InvalidUtf8 => "invalid_utf8",
            Self::Overflow => "overflow",
            Self::BodyError => "body_error",
        }
    }

    const fn expected(self) -> Observation {
        match self {
            Self::Empty => Observation::Success {
                length: 0,
                hash: Fingerprint::OFFSET,
            },
            Self::Single => Observation::success(SINGLE_TEXT.as_bytes()),
            Self::Split => Observation::success(b"split-body"),
            Self::ExactLimit => Observation::success(&AT_LIMIT),
            Self::InvalidUtf8 => Observation::InvalidUtf8 {
                status: StatusCode::BAD_REQUEST,
                valid_up_to: 5,
                error_len: Some(1),
            },
            Self::Overflow => Observation::TooLarge {
                status: StatusCode::PAYLOAD_TOO_LARGE,
                limit: BODY_LIMIT,
                received: BODY_LIMIT + 1,
            },
            Self::BodyError => Observation::Transport {
                status: StatusCode::BAD_REQUEST,
                error: FixtureError(TRANSPORT_FAILURE),
            },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Fingerprint;

impl Fingerprint {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;

    const fn of(bytes: &[u8]) -> u64 {
        let mut hash = Self::OFFSET;
        let mut index = 0;
        while index < bytes.len() {
            hash ^= bytes[index] as u64;
            hash = hash.wrapping_mul(Self::PRIME);
            index += 1;
        }
        hash
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Observation {
    Success {
        length: usize,
        hash: u64,
    },
    InvalidUtf8 {
        status: StatusCode,
        valid_up_to: usize,
        error_len: Option<usize>,
    },
    TooLarge {
        status: StatusCode,
        limit: usize,
        received: usize,
    },
    TooManyFrames {
        status: StatusCode,
        limit: usize,
        received: usize,
    },
    Transport {
        status: StatusCode,
        error: FixtureError,
    },
}

impl Observation {
    const fn success(bytes: &[u8]) -> Self {
        Self::Success {
            length: bytes.len(),
            hash: Fingerprint::of(bytes),
        }
    }
}

struct PreparedScenario {
    parts: http::request::Parts,
    body: FixtureBody,
}

fn prepare(scenario: Scenario) -> PreparedScenario {
    let (parts, ()) = Request::new(()).into_parts();
    PreparedScenario {
        parts,
        body: FixtureBody::from_scenario(scenario),
    }
}

#[expect(clippy::panic, reason = "a pending in-memory extraction future is a benchmark invariant violation")]
fn run_ready<F: Future>(future: F) -> F::Output {
    // Stack-pin to avoid allocator noise on the measured extraction path.
    let mut future = pin!(future);
    let mut context = Context::from_waker(Waker::noop());
    match future.as_mut().poll(&mut context) {
        Poll::Ready(output) => output,
        Poll::Pending => panic!("the in-memory body extraction must complete in one poll"),
    }
}

fn observe_rejection(rejection: BodyRejection<FixtureError>) -> Observation {
    let status = rejection.into_response().status();
    match rejection {
        BodyRejection::InvalidUtf8(error) => Observation::InvalidUtf8 {
            status,
            valid_up_to: error.valid_up_to(),
            error_len: error.error_len(),
        },
        BodyRejection::TooLarge(error) => Observation::TooLarge {
            status,
            limit: error.limit(),
            received: error.received(),
        },
        BodyRejection::TooManyFrames(error) => Observation::TooManyFrames {
            status,
            limit: error.limit(),
            received: error.received(),
        },
        BodyRejection::Transport(error) => Observation::Transport {
            status,
            error: error.into_inner(),
        },
    }
}

fn run_text_prepared(prepared: PreparedScenario) -> Observation {
    match run_ready(TextBody::<BODY_LIMIT>::from_request_body(&prepared.parts, prepared.body, &())) {
        Ok(text) => Observation::success(text.as_bytes()),
        Err(rejection) => observe_rejection(rejection),
    }
}

fn run_utf8_prepared(prepared: PreparedScenario) -> Observation {
    match run_ready(Utf8Body::<BODY_LIMIT>::from_request_body(&prepared.parts, prepared.body, &())) {
        Ok(text) => Observation::success(text.as_str().as_bytes()),
        Err(rejection) => observe_rejection(rejection),
    }
}

fn assert_equivalent() {
    for scenario in Scenario::ALL {
        let expected = scenario.expected();
        let text = run_text_prepared(prepare(scenario));
        let utf8 = run_utf8_prepared(prepare(scenario));
        assert_eq!(
            text,
            expected,
            "TextBody/{} changed its extracted value or rejection",
            scenario.name()
        );
        assert_eq!(utf8, expected, "Utf8Body/{} differs from TextBody", scenario.name());
    }
}

fn assert_utf8_api_retains_single_frame() {
    let transport = Bytes::from_static(SINGLE_TEXT.as_bytes());
    let transport_pointer = transport.as_ptr();
    let (parts, ()) = Request::new(()).into_parts();
    let extracted = run_ready(Utf8Body::<BODY_LIMIT>::from_request_body(
        &parts,
        FixtureBody::from_single(transport),
        &(),
    ))
    .expect("the static frame contains valid bounded UTF-8");

    assert_eq!(extracted.as_str(), SINGLE_TEXT);
    assert_eq!(&*extracted, SINGLE_TEXT);
    let bytes = extracted.into_inner();
    assert_eq!(bytes.as_ptr(), transport_pointer);
    assert_eq!(bytes.as_ref(), SINGLE_TEXT.as_bytes());
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AllocationStats {
    allocations: u64,
    bytes: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AllocationComparison {
    text: AllocationStats,
    utf8: AllocationStats,
}

fn measure_allocations(run: impl FnOnce() -> Observation) -> AllocationStats {
    let session = alloc_tracker::Session::new().no_stdout().no_file();
    let operation = session.operation("extraction");
    {
        let _span = operation.measure_thread().iterations(1);
        std::hint::black_box(run());
    }
    let report = session.to_report();
    let (_, operation) = report
        .operations()
        .find(|(name, _)| *name == "extraction")
        .expect("the extraction allocation operation is recorded");
    AllocationStats {
        allocations: operation.total_allocations_count(),
        bytes: operation.total_bytes_allocated(),
    }
}

fn allocation_diagnostics() -> [(Scenario, AllocationComparison); 7] {
    Scenario::ALL.map(|scenario| {
        let text = std::hint::black_box(prepare(scenario));
        let text = measure_allocations(|| run_text_prepared(text));
        let utf8 = std::hint::black_box(prepare(scenario));
        let utf8 = measure_allocations(|| run_utf8_prepared(utf8));
        (scenario, AllocationComparison { text, utf8 })
    })
}
