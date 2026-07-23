// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Correctness and allocation contracts for response-template candidates.

#![cfg(not(miri))]
#![allow(dead_code, reason = "the shared fixture supports two harnesses and regular tests")]
#![allow(missing_docs, reason = "fixture tests need no API documentation")]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use alloc_tracker::Allocator;

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

include!("../benches/common/response_template_scenarios.rs");

#[test]
fn all_template_and_head_controls_preserve_documented_output() {
    assert_equivalent();
}

#[test]
fn allocation_boundaries_cover_every_template_and_header_cardinality() {
    for (representation, diagnostics) in Representation::ALL.into_iter().zip(body_allocation_diagnostics()) {
        for (scenario, stats) in diagnostics {
            match (representation, scenario) {
                (Representation::ExistingContiguous, _) => assert!(
                    stats.allocations > 0,
                    "{} must retain its measured baseline allocation",
                    scenario.name()
                ),
                (Representation::ExactContiguous | Representation::Segmented, BodyScenario::FullyStatic) => {
                    assert_eq!(stats.allocations, 0, "{} must be allocation-free", representation.name());
                    assert_eq!(stats.bytes, 0);
                }
                (Representation::ExactContiguous | Representation::Segmented, _) => {
                    assert_eq!(
                        stats.allocations,
                        1,
                        "{} / {} must allocate exactly once",
                        representation.name(),
                        scenario.name()
                    );
                }
            }
        }
    }
    for (scenario, stats) in head_allocation_diagnostics() {
        if scenario == HeadScenario::Headers0 {
            assert_eq!(stats.allocations, 0);
        } else {
            assert!(stats.allocations > 0, "{} must expose owned HeaderMap storage", scenario.name());
        }
    }

    let diagnostics = head_candidate_allocation_diagnostics();
    for negotiated_index in 0..2 {
        let ordinary = diagnostics[0][negotiated_index];
        for (representation, candidate) in HeadRepresentation::ALL.into_iter().zip(diagnostics).skip(1) {
            for ((scenario, baseline), (_, candidate)) in ordinary.into_iter().zip(candidate[negotiated_index]) {
                assert!(
                    candidate.allocations <= baseline.allocations && candidate.bytes <= baseline.bytes,
                    "{} / {} / negotiated={} allocated {}/{} bytes versus ordinary {}/{} bytes",
                    representation.name(),
                    scenario.name(),
                    negotiated_index == 1,
                    candidate.allocations,
                    candidate.bytes,
                    baseline.allocations,
                    baseline.bytes
                );
                if scenario != HeadScenario::Headers0 || negotiated_index == 1 {
                    assert!(
                        candidate.allocations > 0,
                        "{} remains an owned HeaderMap CPU optimization",
                        representation.name()
                    );
                }
            }
        }
    }
}

#[test]
fn typed_slots_apply_json_and_html_escaping() {
    let json_value = "quote: \"x\"\ncontrol:\u{0008} snowman: \u{2603}";
    let expected_json = format!(
        "{{\"message\":{}}}",
        serde_json::to_string(json_value).expect("serializing the expected JSON string succeeds")
    );
    assert_eq!(collect(exact_json_string(JsonString(json_value))), expected_json.as_bytes());
    assert_eq!(collect(segmented_json_string(JsonString(json_value))), expected_json.as_bytes());

    let mut every_json_escape = String::from("\"\\/");
    every_json_escape.extend((0_u8..=0x1f).map(char::from));
    every_json_escape.push('\u{2603}');
    let expected_every_json_escape = format!(
        "{{\"message\":{}}}",
        serde_json::to_string(&every_json_escape).expect("serializing every JSON escape succeeds")
    );
    assert_eq!(
        collect(exact_json_string(JsonString(&every_json_escape))),
        expected_every_json_escape.as_bytes()
    );
    assert_eq!(
        collect(segmented_json_string(JsonString(&every_json_escape))),
        expected_every_json_escape.as_bytes()
    );

    let html_value = "<script>\"x\" & 'y'</script>";
    let expected_html = format!("{MEDIUM_PREFIX}&lt;script&gt;&quot;x&quot; &amp; &#39;y&#39;&lt;/script&gt;{MEDIUM_SUFFIX}");
    assert_eq!(collect(exact_html_text(HtmlText(html_value))), expected_html.as_bytes());
    assert_eq!(collect(segmented_html_text(HtmlText(html_value))), expected_html.as_bytes());

    let integer_boundaries = json_body_template!(
        minimum = number(i128::MIN),
        maximum = number(u128::MAX);
        "[", minimum, ",", maximum, "]"
    );
    assert_eq!(integer_boundaries.as_bytes(), format!("[{},{}]", i128::MIN, u128::MAX).as_bytes());

    let evaluations = std::cell::Cell::new(0);
    let once = json_body_template!(
        value = number({
            evaluations.set(evaluations.get() + 1);
            42_u64
        });
        r#"{"value":"#, value, "}"
    );
    assert_eq!(once.as_bytes(), br#"{"value":42}"#);
    assert_eq!(evaluations.get(), 1);
}

#[test]
fn segmented_dynamic_ownership_drops_before_or_after_polling() {
    let dropped_unpolled = Arc::new(AtomicBool::new(false));
    let body = SegmentedBody::new(
        b"prefix",
        Some(Bytes::from_owner(TrackedOwner::new(b"dynamic", Arc::clone(&dropped_unpolled)))),
        b"suffix",
    );
    drop(body);
    assert!(dropped_unpolled.load(Ordering::Relaxed));

    let dropped_transferred = Arc::new(AtomicBool::new(false));
    let mut body = Box::pin(SegmentedBody::new(
        b"prefix",
        Some(Bytes::from_owner(TrackedOwner::new(b"dynamic", Arc::clone(&dropped_transferred)))),
        b"suffix",
    ));
    let mut context = Context::from_waker(Waker::noop());
    let prefix = match body.as_mut().poll_frame(&mut context) {
        Poll::Ready(Some(Ok(frame))) => frame.into_data().expect("the first frame is static data"),
        _ => panic!("the first segmented frame must be ready"),
    };
    drop(prefix);
    let dynamic = match body.as_mut().poll_frame(&mut context) {
        Poll::Ready(Some(Ok(frame))) => frame.into_data().expect("the second frame is dynamic data"),
        _ => panic!("the second segmented frame must be ready"),
    };
    drop(body);
    assert!(!dropped_transferred.load(Ordering::Relaxed));
    drop(dynamic);
    assert!(dropped_transferred.load(Ordering::Relaxed));
}

#[test]
fn exact_template_drops_owned_slots_after_rendering() {
    let dropped = Arc::new(AtomicBool::new(false));
    let body = json_body_template!(
        message = string(TrackedText {
            text: "owned",
            dropped: Arc::clone(&dropped),
        });
        r#"{"message":"#, message, "}"
    );
    assert!(dropped.load(Ordering::Relaxed));
    assert_eq!(body.as_bytes(), br#"{"message":"owned"}"#);
}

#[expect(clippy::panic, reason = "a pending typed response-template body is a fixture invariant violation")]
fn collect<B>(body: B) -> Vec<u8>
where
    B: http_body::Body<Data = Bytes, Error = std::convert::Infallible>,
{
    let mut body = std::pin::pin!(body);
    let mut context = Context::from_waker(Waker::noop());
    let mut output = Vec::new();
    loop {
        match body.as_mut().poll_frame(&mut context) {
            Poll::Ready(Some(Ok(frame))) => {
                output.extend_from_slice(&frame.into_data().expect("response-template candidates emit data frames only"));
            }
            Poll::Ready(Some(Err(error))) => match error {},
            Poll::Ready(None) => return output,
            Poll::Pending => panic!("response-template candidates are always ready"),
        }
    }
}

struct TrackedOwner {
    bytes: &'static [u8],
    dropped: Arc<AtomicBool>,
}

struct TrackedText {
    text: &'static str,
    dropped: Arc<AtomicBool>,
}

impl AsRef<str> for TrackedText {
    fn as_ref(&self) -> &str {
        self.text
    }
}

impl Drop for TrackedText {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::Relaxed);
    }
}

impl TrackedOwner {
    fn new(bytes: &'static [u8], dropped: Arc<AtomicBool>) -> Self {
        Self { bytes, dropped }
    }
}

impl AsRef<[u8]> for TrackedOwner {
    fn as_ref(&self) -> &[u8] {
        self.bytes
    }
}

impl Drop for TrackedOwner {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::Relaxed);
    }
}
