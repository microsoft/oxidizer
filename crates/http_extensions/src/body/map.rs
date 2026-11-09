// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Maps existing [`HttpBody`] content while preserving its policies.
//!
//! See [`HttpBody::map`] for more information.
//!
//! If you want to create a new body, use [`HttpBodyBuilder`][crate::HttpBodyBuilder].

use std::pin::Pin;
use std::task::{Context, Poll};

use bytesbuf::BytesView;
use http_body::{Body, Frame, SizeHint};
use http_body_util::BodyExt;
use thread_aware::ThreadAware;

use super::{HttpBody, Kind};
use crate::error_labels::LABEL_BODY_CONSUMED;
use crate::{HttpError, Result};

pub(crate) fn map_body(body: HttpBody, map_content: impl FnOnce(Content) -> Result<Content>) -> Result<HttpBody> {
    let options = body.options();
    let clock = body.clock;
    let (content, current_delay) = match body.kind {
        Kind::Body(inner, current_delay) => (Content::Streaming(inner), current_delay),
        Kind::Bytes(Some(bytes)) => (Content::Bytes(bytes), None),
        Kind::Empty => (Content::Bytes(BytesView::default()), None),
        Kind::Bytes(None) => {
            return Err(HttpError::validation_with_label(
                "body cannot be transformed because it is already consumed",
                LABEL_BODY_CONSUMED,
            ));
        }
    };

    match map_content(content)? {
        Content::Streaming(inner) => Ok(HttpBody::from_streaming(inner, options, clock, current_delay)),
        Content::Bytes(bytes) => Ok(HttpBody::from_bytes(bytes, options, clock)),
    }
}

/// Raw content passed to and returned from [`HttpBody::map`].
///
/// Match both variants to map buffered bytes and streaming frames.
#[must_use]
#[expect(clippy::large_enum_variant, reason = "keeps buffered bytes inline without an extra allocation")]
#[derive(derive_more::Debug)]
pub enum Content {
    /// Buffered bytes. An empty body supplies an empty view.
    #[debug("Bytes")]
    Bytes(BytesView),
    /// A streaming body without the surrounding `HttpBody` policies.
    Streaming(StreamingBody),
}

/// Streaming content used by [`HttpBody::map`].
///
/// The transformation preserves the body's policies around this content.
#[derive(ThreadAware, derive_more::Debug)]
#[must_use]
pub struct StreamingBody {
    #[thread_aware(skip)]
    #[debug(skip)]
    inner: Pin<Box<dyn Body<Data = BytesView, Error = HttpError> + Send>>,
}

impl StreamingBody {
    /// Stores a body and converts its errors to [`HttpError`].
    pub fn new<B>(body: B) -> Self
    where
        B: Body<Data = BytesView, Error: Into<HttpError>> + Send + 'static,
    {
        Self {
            inner: Box::pin(body.map_err(Into::into)),
        }
    }
}

impl Body for StreamingBody {
    type Data = BytesView;
    type Error = HttpError;

    fn poll_frame(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Self::Data>>>> {
        self.get_mut().inner.as_mut().poll_frame(cx)
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }
}
