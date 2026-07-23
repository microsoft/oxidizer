// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

// Shared workload for isolated response-template compile and section controls.
// Each target defines one `render` function and body type before including it.

use http_body::Body as HttpBody;

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

#[derive(Clone, Copy)]
enum Scenario {
    FullyStatic,
    NumericJson,
    EscapedJson,
    MediumTextShell,
}

impl Scenario {
    const ALL: [Self; 4] = [
        Self::FullyStatic,
        Self::NumericJson,
        Self::EscapedJson,
        Self::MediumTextShell,
    ];

    const fn expected(self) -> &'static [u8] {
        match self {
            Self::FullyStatic => FULLY_STATIC.as_bytes(),
            Self::NumericJson => NUMERIC_JSON.as_bytes(),
            Self::EscapedJson => ESCAPED_JSON.as_bytes(),
            Self::MediumTextShell => MEDIUM_EXPECTED.as_bytes(),
        }
    }
}

#[expect(
    clippy::panic,
    reason = "a pending or failed isolated response-template body is a benchmark invariant violation"
)]
fn observe<B>(body: B) -> (usize, u64)
where
    B: HttpBody<Data = bytes::Bytes>,
    B::Error: std::fmt::Debug,
{
    let mut body = std::pin::pin!(body);
    let mut context = std::task::Context::from_waker(std::task::Waker::noop());
    let mut length = 0;
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    loop {
        match body.as_mut().poll_frame(&mut context) {
            std::task::Poll::Ready(Some(Ok(frame))) => {
                let data = frame.into_data().expect("size-control templates emit data frames only");
                length += data.len();
                for byte in data {
                    hash ^= u64::from(byte);
                    hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
                }
            }
            std::task::Poll::Ready(Some(Err(error))) => panic!("the size-control template failed: {error:?}"),
            std::task::Poll::Ready(None) => return (length, hash),
            std::task::Poll::Pending => panic!("the size-control template is always ready"),
        }
    }
}

fn main() {
    for scenario in Scenario::ALL {
        let observation = observe(render(scenario));
        let expected = observe(routerama::response::Body::from(bytes::Bytes::copy_from_slice(
            scenario.expected(),
        )));
        assert_eq!(observation, expected);
        std::hint::black_box(observation);
    }
}
