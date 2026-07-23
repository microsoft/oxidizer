// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

// Generated overlap groups used to measure repeated request-predicate parsing.
// Request construction is excluded; measured work covers routing and complete
// response observation. Candidate order and rejection precedence are asserted.

use std::pin::pin;
use std::task::{Context, Poll, Waker};

use bytes::Bytes;
use http::{HeaderValue, Request};
use http_body::Body as HttpBody;
use routerama::route::router;

type Observation = (u16, Option<u8>, usize);

macro_rules! media_type {
    ($index:expr) => {
        match $index {
            0 => "application/x-routerama-00",
            1 => "application/x-routerama-01",
            2 => "application/x-routerama-02",
            3 => "application/x-routerama-03",
            4 => "application/x-routerama-04",
            5 => "application/x-routerama-05",
            6 => "application/x-routerama-06",
            7 => "application/x-routerama-07",
            8 => "application/x-routerama-08",
            9 => "application/x-routerama-09",
            10 => "application/x-routerama-10",
            11 => "application/x-routerama-11",
            12 => "application/x-routerama-12",
            13 => "application/x-routerama-13",
            14 => "application/x-routerama-14",
            15 => "application/x-routerama-15",
            16 => "application/x-routerama-16",
            17 => "application/x-routerama-17",
            18 => "application/x-routerama-18",
            19 => "application/x-routerama-19",
            20 => "application/x-routerama-20",
            21 => "application/x-routerama-21",
            22 => "application/x-routerama-22",
            23 => "application/x-routerama-23",
            24 => "application/x-routerama-24",
            25 => "application/x-routerama-25",
            26 => "application/x-routerama-26",
            27 => "application/x-routerama-27",
            28 => "application/x-routerama-28",
            29 => "application/x-routerama-29",
            30 => "application/x-routerama-30",
            31 => "application/x-routerama-31",
            _ => panic!("predicate fixture candidate index is out of range"),
        }
    };
}

struct Overlap2;

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "router handlers must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[router]
impl Overlap2 {
    #[route(
        POST,
        "/overlap",
        host = "api.example",
        consumes = "application/json",
        produces = "application/x-routerama-00",
        priority = 32
    )]
    async fn candidate_00(&self) -> Bytes {
        Bytes::from_static(&[0])
    }
    #[route(
        POST,
        "/overlap",
        host = "api.example",
        consumes = "application/json",
        produces = "application/x-routerama-01",
        priority = 31
    )]
    async fn candidate_01(&self) -> Bytes {
        Bytes::from_static(&[1])
    }
}

struct Overlap8;

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "router handlers must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[router]
impl Overlap8 {
    #[route(
        POST,
        "/overlap",
        host = "api.example",
        consumes = "application/json",
        produces = "application/x-routerama-00",
        priority = 32
    )]
    async fn candidate_00(&self) -> Bytes {
        Bytes::from_static(&[0])
    }
    #[route(
        POST,
        "/overlap",
        host = "api.example",
        consumes = "application/json",
        produces = "application/x-routerama-01",
        priority = 31
    )]
    async fn candidate_01(&self) -> Bytes {
        Bytes::from_static(&[1])
    }
    #[route(
        POST,
        "/overlap",
        host = "api.example",
        consumes = "application/json",
        produces = "application/x-routerama-02",
        priority = 30
    )]
    async fn candidate_02(&self) -> Bytes {
        Bytes::from_static(&[2])
    }
    #[route(
        POST,
        "/overlap",
        host = "api.example",
        consumes = "application/json",
        produces = "application/x-routerama-03",
        priority = 29
    )]
    async fn candidate_03(&self) -> Bytes {
        Bytes::from_static(&[3])
    }
    #[route(
        POST,
        "/overlap",
        host = "api.example",
        consumes = "application/json",
        produces = "application/x-routerama-04",
        priority = 28
    )]
    async fn candidate_04(&self) -> Bytes {
        Bytes::from_static(&[4])
    }
    #[route(
        POST,
        "/overlap",
        host = "api.example",
        consumes = "application/json",
        produces = "application/x-routerama-05",
        priority = 27
    )]
    async fn candidate_05(&self) -> Bytes {
        Bytes::from_static(&[5])
    }
    #[route(
        POST,
        "/overlap",
        host = "api.example",
        consumes = "application/json",
        produces = "application/x-routerama-06",
        priority = 26
    )]
    async fn candidate_06(&self) -> Bytes {
        Bytes::from_static(&[6])
    }
    #[route(
        POST,
        "/overlap",
        host = "api.example",
        consumes = "application/json",
        produces = "application/x-routerama-07",
        priority = 25
    )]
    async fn candidate_07(&self) -> Bytes {
        Bytes::from_static(&[7])
    }
}

struct Overlap32;

#[allow(
    clippy::allow_attributes,
    unknown_lints,
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    reason = "router handlers must be async; `clippy::unused_async_trait_impl` does not exist before clippy 0.1.98"
)]
#[router]
impl Overlap32 {
    #[route(
        POST,
        "/overlap",
        host = "api.example",
        consumes = "application/json",
        produces = "application/x-routerama-00",
        priority = 32
    )]
    async fn candidate_00(&self) -> Bytes {
        Bytes::from_static(&[0])
    }
    #[route(
        POST,
        "/overlap",
        host = "api.example",
        consumes = "application/json",
        produces = "application/x-routerama-01",
        priority = 31
    )]
    async fn candidate_01(&self) -> Bytes {
        Bytes::from_static(&[1])
    }
    #[route(
        POST,
        "/overlap",
        host = "api.example",
        consumes = "application/json",
        produces = "application/x-routerama-02",
        priority = 30
    )]
    async fn candidate_02(&self) -> Bytes {
        Bytes::from_static(&[2])
    }
    #[route(
        POST,
        "/overlap",
        host = "api.example",
        consumes = "application/json",
        produces = "application/x-routerama-03",
        priority = 29
    )]
    async fn candidate_03(&self) -> Bytes {
        Bytes::from_static(&[3])
    }
    #[route(
        POST,
        "/overlap",
        host = "api.example",
        consumes = "application/json",
        produces = "application/x-routerama-04",
        priority = 28
    )]
    async fn candidate_04(&self) -> Bytes {
        Bytes::from_static(&[4])
    }
    #[route(
        POST,
        "/overlap",
        host = "api.example",
        consumes = "application/json",
        produces = "application/x-routerama-05",
        priority = 27
    )]
    async fn candidate_05(&self) -> Bytes {
        Bytes::from_static(&[5])
    }
    #[route(
        POST,
        "/overlap",
        host = "api.example",
        consumes = "application/json",
        produces = "application/x-routerama-06",
        priority = 26
    )]
    async fn candidate_06(&self) -> Bytes {
        Bytes::from_static(&[6])
    }
    #[route(
        POST,
        "/overlap",
        host = "api.example",
        consumes = "application/json",
        produces = "application/x-routerama-07",
        priority = 25
    )]
    async fn candidate_07(&self) -> Bytes {
        Bytes::from_static(&[7])
    }
    #[route(
        POST,
        "/overlap",
        host = "api.example",
        consumes = "application/json",
        produces = "application/x-routerama-08",
        priority = 24
    )]
    async fn candidate_08(&self) -> Bytes {
        Bytes::from_static(&[8])
    }
    #[route(
        POST,
        "/overlap",
        host = "api.example",
        consumes = "application/json",
        produces = "application/x-routerama-09",
        priority = 23
    )]
    async fn candidate_09(&self) -> Bytes {
        Bytes::from_static(&[9])
    }
    #[route(
        POST,
        "/overlap",
        host = "api.example",
        consumes = "application/json",
        produces = "application/x-routerama-10",
        priority = 22
    )]
    async fn candidate_10(&self) -> Bytes {
        Bytes::from_static(&[10])
    }
    #[route(
        POST,
        "/overlap",
        host = "api.example",
        consumes = "application/json",
        produces = "application/x-routerama-11",
        priority = 21
    )]
    async fn candidate_11(&self) -> Bytes {
        Bytes::from_static(&[11])
    }
    #[route(
        POST,
        "/overlap",
        host = "api.example",
        consumes = "application/json",
        produces = "application/x-routerama-12",
        priority = 20
    )]
    async fn candidate_12(&self) -> Bytes {
        Bytes::from_static(&[12])
    }
    #[route(
        POST,
        "/overlap",
        host = "api.example",
        consumes = "application/json",
        produces = "application/x-routerama-13",
        priority = 19
    )]
    async fn candidate_13(&self) -> Bytes {
        Bytes::from_static(&[13])
    }
    #[route(
        POST,
        "/overlap",
        host = "api.example",
        consumes = "application/json",
        produces = "application/x-routerama-14",
        priority = 18
    )]
    async fn candidate_14(&self) -> Bytes {
        Bytes::from_static(&[14])
    }
    #[route(
        POST,
        "/overlap",
        host = "api.example",
        consumes = "application/json",
        produces = "application/x-routerama-15",
        priority = 17
    )]
    async fn candidate_15(&self) -> Bytes {
        Bytes::from_static(&[15])
    }
    #[route(
        POST,
        "/overlap",
        host = "api.example",
        consumes = "application/json",
        produces = "application/x-routerama-16",
        priority = 16
    )]
    async fn candidate_16(&self) -> Bytes {
        Bytes::from_static(&[16])
    }
    #[route(
        POST,
        "/overlap",
        host = "api.example",
        consumes = "application/json",
        produces = "application/x-routerama-17",
        priority = 15
    )]
    async fn candidate_17(&self) -> Bytes {
        Bytes::from_static(&[17])
    }
    #[route(
        POST,
        "/overlap",
        host = "api.example",
        consumes = "application/json",
        produces = "application/x-routerama-18",
        priority = 14
    )]
    async fn candidate_18(&self) -> Bytes {
        Bytes::from_static(&[18])
    }
    #[route(
        POST,
        "/overlap",
        host = "api.example",
        consumes = "application/json",
        produces = "application/x-routerama-19",
        priority = 13
    )]
    async fn candidate_19(&self) -> Bytes {
        Bytes::from_static(&[19])
    }
    #[route(
        POST,
        "/overlap",
        host = "api.example",
        consumes = "application/json",
        produces = "application/x-routerama-20",
        priority = 12
    )]
    async fn candidate_20(&self) -> Bytes {
        Bytes::from_static(&[20])
    }
    #[route(
        POST,
        "/overlap",
        host = "api.example",
        consumes = "application/json",
        produces = "application/x-routerama-21",
        priority = 11
    )]
    async fn candidate_21(&self) -> Bytes {
        Bytes::from_static(&[21])
    }
    #[route(
        POST,
        "/overlap",
        host = "api.example",
        consumes = "application/json",
        produces = "application/x-routerama-22",
        priority = 10
    )]
    async fn candidate_22(&self) -> Bytes {
        Bytes::from_static(&[22])
    }
    #[route(
        POST,
        "/overlap",
        host = "api.example",
        consumes = "application/json",
        produces = "application/x-routerama-23",
        priority = 9
    )]
    async fn candidate_23(&self) -> Bytes {
        Bytes::from_static(&[23])
    }
    #[route(
        POST,
        "/overlap",
        host = "api.example",
        consumes = "application/json",
        produces = "application/x-routerama-24",
        priority = 8
    )]
    async fn candidate_24(&self) -> Bytes {
        Bytes::from_static(&[24])
    }
    #[route(
        POST,
        "/overlap",
        host = "api.example",
        consumes = "application/json",
        produces = "application/x-routerama-25",
        priority = 7
    )]
    async fn candidate_25(&self) -> Bytes {
        Bytes::from_static(&[25])
    }
    #[route(
        POST,
        "/overlap",
        host = "api.example",
        consumes = "application/json",
        produces = "application/x-routerama-26",
        priority = 6
    )]
    async fn candidate_26(&self) -> Bytes {
        Bytes::from_static(&[26])
    }
    #[route(
        POST,
        "/overlap",
        host = "api.example",
        consumes = "application/json",
        produces = "application/x-routerama-27",
        priority = 5
    )]
    async fn candidate_27(&self) -> Bytes {
        Bytes::from_static(&[27])
    }
    #[route(
        POST,
        "/overlap",
        host = "api.example",
        consumes = "application/json",
        produces = "application/x-routerama-28",
        priority = 4
    )]
    async fn candidate_28(&self) -> Bytes {
        Bytes::from_static(&[28])
    }
    #[route(
        POST,
        "/overlap",
        host = "api.example",
        consumes = "application/json",
        produces = "application/x-routerama-29",
        priority = 3
    )]
    async fn candidate_29(&self) -> Bytes {
        Bytes::from_static(&[29])
    }
    #[route(
        POST,
        "/overlap",
        host = "api.example",
        consumes = "application/json",
        produces = "application/x-routerama-30",
        priority = 2
    )]
    async fn candidate_30(&self) -> Bytes {
        Bytes::from_static(&[30])
    }
    #[route(
        POST,
        "/overlap",
        host = "api.example",
        consumes = "application/json",
        produces = "application/x-routerama-31",
        priority = 1
    )]
    async fn candidate_31(&self) -> Bytes {
        Bytes::from_static(&[31])
    }
}

static OVERLAP_2: Overlap2 = Overlap2;
static OVERLAP_8: Overlap8 = Overlap8;
static OVERLAP_32: Overlap32 = Overlap32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GroupSize {
    Two,
    Eight,
    ThirtyTwo,
}

impl GroupSize {
    const ALL: [Self; 3] = [Self::Two, Self::Eight, Self::ThirtyTwo];

    const fn count(self) -> u8 {
        match self {
            Self::Two => 2,
            Self::Eight => 8,
            Self::ThirtyTwo => 32,
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::Two => "overlap_2",
            Self::Eight => "overlap_8",
            Self::ThirtyTwo => "overlap_32",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scenario {
    First,
    Middle,
    Last,
    Miss,
    MalformedAccept,
    MultipleAccept,
    MultipleContentType,
    MultipleHost,
}

impl Scenario {
    const ALL: [Self; 8] = [
        Self::First,
        Self::Middle,
        Self::Last,
        Self::Miss,
        Self::MalformedAccept,
        Self::MultipleAccept,
        Self::MultipleContentType,
        Self::MultipleHost,
    ];

    const fn name(self) -> &'static str {
        match self {
            Self::First => "winner_first",
            Self::Middle => "winner_middle",
            Self::Last => "winner_last",
            Self::Miss => "miss",
            Self::MalformedAccept => "malformed_accept",
            Self::MultipleAccept => "multiple_accept",
            Self::MultipleContentType => "multiple_content_type",
            Self::MultipleHost => "multiple_host",
        }
    }

    const fn winner(self, size: GroupSize) -> Option<u8> {
        match self {
            Self::First | Self::MultipleAccept => Some(0),
            Self::Middle => Some(size.count() / 2),
            Self::Last => Some(size.count() - 1),
            Self::Miss
            | Self::MalformedAccept
            | Self::MultipleContentType
            | Self::MultipleHost => None,
        }
    }

    const fn expected(self, size: GroupSize) -> Observation {
        if let Some(winner) = self.winner(size) {
            (200, Some(winner), 1)
        } else {
            let status = match self {
                Self::MultipleContentType => 415,
                Self::MultipleHost => 404,
                Self::Miss | Self::MalformedAccept => 406,
                Self::First | Self::Middle | Self::Last | Self::MultipleAccept => 200,
            };
            (status, None, 0)
        }
    }
}

type PreparedScenario = Box<(GroupSize, Request<()>)>;

fn prepare(size: GroupSize, scenario: Scenario) -> PreparedScenario {
    let winner = scenario.winner(size).unwrap_or(0);
    let accept = match scenario {
        Scenario::Miss => "text/plain",
        Scenario::MalformedAccept => "application/",
        Scenario::First
        | Scenario::Middle
        | Scenario::Last
        | Scenario::MultipleAccept
        | Scenario::MultipleContentType
        | Scenario::MultipleHost => media_type!(winner),
    };
    let mut request = Request::builder()
        .method("POST")
        .uri("/overlap")
        .header("host", "api.example")
        .header("content-type", "application/json")
        .header("accept", HeaderValue::from_static(accept))
        .body(())
        .expect("predicate overlap request metadata is valid");
    match scenario {
        Scenario::MultipleAccept => {
            request
                .headers_mut()
                .append("accept", HeaderValue::from_static("text/plain"));
        }
        Scenario::MultipleContentType => {
            request
                .headers_mut()
                .append("content-type", HeaderValue::from_static("application/json"));
        }
        Scenario::MultipleHost => {
            request
                .headers_mut()
                .append("host", HeaderValue::from_static("api.example"));
        }
        Scenario::First | Scenario::Middle | Scenario::Last | Scenario::Miss | Scenario::MalformedAccept => {}
    }
    Box::new((size, request))
}

#[expect(
    clippy::panic,
    reason = "a pending in-memory route future is a benchmark invariant violation"
)]
fn run_ready<F: Future>(future: F) -> F::Output {
    // Stack-pin to avoid allocator noise on the measured route path.
    let mut future = pin!(future);
    let mut context = Context::from_waker(Waker::noop());
    match future.as_mut().poll(&mut context) {
        Poll::Ready(output) => output,
        Poll::Pending => panic!("the in-memory overlap route must complete in one poll"),
    }
}

#[expect(
    clippy::panic,
    reason = "pending or failed fixture response bodies are benchmark invariant violations"
)]
fn observe<B>(response: http::Response<B>) -> Observation
where
    B: HttpBody<Data = Bytes>,
{
    let status = response.status().as_u16();
    let mut winner = None;
    let mut length = 0;
    // Stack-pin to keep body polling allocation-free on the measured path.
    let mut body = pin!(response.into_body());
    let mut context = Context::from_waker(Waker::noop());
    loop {
        match body.as_mut().poll_frame(&mut context) {
            Poll::Ready(Some(Ok(frame))) => {
                if let Some(data) = frame.data_ref() {
                    length += data.len();
                    winner = winner.or_else(|| data.first().copied());
                }
            }
            Poll::Ready(Some(Err(_))) => panic!("predicate fixture response bodies never fail"),
            Poll::Ready(None) => break,
            Poll::Pending => panic!("predicate fixture response bodies are always ready"),
        }
    }
    (status, winner, length)
}

fn run_prepared(prepared: PreparedScenario) -> Observation {
    let (size, request) = *prepared;
    match size {
        GroupSize::Two => observe(run_ready(OVERLAP_2.route(request, &()))),
        GroupSize::Eight => observe(run_ready(OVERLAP_8.route(request, &()))),
        GroupSize::ThirtyTwo => observe(run_ready(OVERLAP_32.route(request, &()))),
    }
}

fn assert_equivalent() {
    for size in GroupSize::ALL {
        for scenario in Scenario::ALL {
            assert_eq!(
                run_prepared(prepare(size, scenario)),
                scenario.expected(size),
                "{}/{} changed winner or rejection precedence",
                size.name(),
                scenario.name()
            );
        }
    }
}
