// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Maps existing [`HttpBody`] content while preserving its policies.
//!
//! See [`HttpBody::map`] for more information.
//! Call it before polling begins. Mapping does not preserve an in-progress timeout.
//!
//! If you want to create a new body, use [`HttpBodyBuilder`][crate::HttpBodyBuilder].

use bytesbuf::BytesView;

use super::body::{Content, HttpBody, Kind};
use crate::error_labels::LABEL_BODY_CONSUMED;
use crate::{HttpError, Result};

pub(crate) fn map_body(body: HttpBody, map_content: impl FnOnce(Content) -> Result<Content>) -> Result<HttpBody> {
    let options = body.options();
    let clock = body.clock;
    let content = match body.kind {
        Kind::Body(inner, _) => Content::Streaming(inner),
        Kind::Bytes(Some(bytes)) => Content::Bytes(bytes),
        Kind::Empty => Content::Bytes(BytesView::default()),
        Kind::Bytes(None) => {
            return Err(HttpError::validation_with_label(
                "body cannot be mapped because it is already consumed",
                LABEL_BODY_CONSUMED,
            ));
        }
    };

    match map_content(content)? {
        Content::Streaming(inner) => Ok(HttpBody::from_streaming(inner, options, clock, None)),
        Content::Bytes(bytes) => Ok(HttpBody::from_bytes(bytes, options, clock)),
    }
}
