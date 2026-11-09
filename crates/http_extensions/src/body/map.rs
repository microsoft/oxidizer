// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Maps existing [`HttpBody`] content while preserving its policies.
//!
//! See [`HttpBody::map`] for more information.
//!
//! If you want to create a new body, use [`HttpBodyBuilder`][crate::HttpBodyBuilder].

use bytesbuf::BytesView;

use super::body::{Content, HttpBody, Kind};
use crate::error_labels::LABEL_BODY_CONSUMED;
use crate::{HttpError, Result};

pub(crate) fn map_body(body: HttpBody, map_content: impl FnOnce(Content) -> Result<Content>) -> Result<HttpBody> {
    let options = body.options();
    let clock = body.clock;
    let (content, mapped_timeout, current_delay) = match body.kind {
        Kind::Body(inner, active_timeout, current_delay) => (Content::Streaming(inner), active_timeout, current_delay),
        Kind::Bytes(Some(bytes)) => (Content::Bytes(bytes), options.timeout, None),
        Kind::Empty => (Content::Bytes(BytesView::default()), options.timeout, None),
        Kind::Bytes(None) => {
            return Err(HttpError::validation_with_label(
                "body cannot be transformed because it is already consumed",
                LABEL_BODY_CONSUMED,
            ));
        }
    };

    match map_content(content)? {
        Content::Streaming(inner) => Ok(HttpBody::from_streaming_with_timeout(
            inner,
            options,
            clock,
            mapped_timeout,
            current_delay,
        )),
        Content::Bytes(bytes) => Ok(HttpBody::from_bytes(bytes, options, clock)),
    }
}
