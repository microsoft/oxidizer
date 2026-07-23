// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Isolated compile and section-size control for segmented response templates.

use std::convert::Infallible;
use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::Bytes;
use http_body::{Frame, SizeHint};
use routerama::response::__template::{html_text, json_number, json_string, slot_len, write_slot};

struct SegmentedBody {
    prefix: &'static [u8],
    dynamic: Option<Bytes>,
    suffix: &'static [u8],
    state: u8,
    remaining: usize,
}

impl SegmentedBody {
    fn new(prefix: &'static [u8], dynamic: Option<Bytes>, suffix: &'static [u8]) -> Self {
        Self {
            prefix,
            remaining: prefix.len() + dynamic.as_ref().map_or(0, Bytes::len) + suffix.len(),
            dynamic,
            suffix,
            state: 0,
        }
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
            self.remaining -= bytes.len();
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

fn render(scenario: Scenario) -> SegmentedBody {
    match scenario {
        Scenario::FullyStatic => SegmentedBody::new(FULLY_STATIC.as_bytes(), None, b""),
        Scenario::NumericJson => {
            let slot = json_number(NUMERIC_ID);
            let mut dynamic = Vec::with_capacity(slot_len(&slot));
            write_slot(&mut dynamic, &slot);
            SegmentedBody::new(NUMERIC_PREFIX, Some(Bytes::from(dynamic)), NUMERIC_SUFFIX)
        }
        Scenario::EscapedJson => {
            let slot = json_string(ESCAPED_MESSAGE);
            let mut dynamic = Vec::with_capacity(slot_len(&slot));
            write_slot(&mut dynamic, &slot);
            SegmentedBody::new(ESCAPED_PREFIX, Some(Bytes::from(dynamic)), ESCAPED_SUFFIX)
        }
        Scenario::MediumTextShell => {
            let slot = html_text(MEDIUM_NAME);
            let mut dynamic = Vec::with_capacity(slot_len(&slot));
            write_slot(&mut dynamic, &slot);
            SegmentedBody::new(MEDIUM_PREFIX.as_bytes(), Some(Bytes::from(dynamic)), MEDIUM_SUFFIX.as_bytes())
        }
    }
}

include!("common/response_template_size_control.rs");
