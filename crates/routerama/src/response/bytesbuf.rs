// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! `bytesbuf` response bodies that preserve scatter/gather storage.

use core::convert::Infallible;
use core::pin::Pin;
use core::task::{Context, Poll};

use bytesbuf::BytesView;
use http_body::{Frame, SizeHint};

use super::{IntoResponse, Response};

/// Prepared typed `BytesView` response templates.
pub mod template;

/// A zero-or-one-frame response body backed by a [`BytesView`].
///
/// The view is yielded unchanged. No payload bytes are copied or coalesced.
#[derive(Clone, Debug, Default)]
pub struct BytesViewBody(Option<BytesView>);

impl BytesViewBody {
    /// Creates an empty body.
    #[must_use]
    pub const fn empty() -> Self {
        Self(None)
    }

    /// Creates a body that yields `view` once.
    #[must_use]
    pub fn new(view: BytesView) -> Self {
        if view.is_empty() { Self::empty() } else { Self(Some(view)) }
    }

    /// Returns the retained view, if the body is not empty.
    #[must_use]
    pub const fn view(&self) -> Option<&BytesView> {
        self.0.as_ref()
    }

    /// Consumes the body and returns its view.
    #[must_use]
    pub fn into_inner(self) -> BytesView {
        self.0.unwrap_or_default()
    }
}

impl From<BytesView> for BytesViewBody {
    fn from(view: BytesView) -> Self {
        Self::new(view)
    }
}

impl http_body::Body for BytesViewBody {
    type Data = BytesView;
    type Error = Infallible;

    fn poll_frame(mut self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        Poll::Ready(self.0.take().map(|view| Ok(Frame::data(view))))
    }

    fn is_end_stream(&self) -> bool {
        self.0.is_none()
    }

    fn size_hint(&self) -> SizeHint {
        let length = self.0.as_ref().map_or(0, BytesView::len) as u64;
        SizeHint::with_exact(length)
    }
}

impl IntoResponse for BytesViewBody {
    type Body = Self;

    fn into_response(self) -> Response<Self::Body> {
        Response::new(self)
    }
}

impl IntoResponse for BytesView {
    type Body = BytesViewBody;

    fn into_response(self) -> Response<Self::Body> {
        BytesViewBody::new(self).into_response()
    }
}
