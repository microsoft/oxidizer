// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::pin::Pin;
use std::task::{Context, Poll};

use bytesbuf::BytesView;
use futures::stream;
use http_body::Frame;

use crate::{HttpBodyBuilder, HttpBodyOptions, HttpError, Result};

pub(crate) fn create_stream_body(builder: &HttpBodyBuilder, body: impl AsRef<[u8]>, options: &HttpBodyOptions) -> crate::HttpBody {
    let data = body.as_ref();
    if data.is_empty() {
        builder
            .clone()
            .with_options(*options)
            .stream(stream::iter(Vec::<Result<BytesView>>::new()))
    } else {
        let chunk = BytesView::copied_from_slice(data, builder);
        builder.clone().with_options(*options).stream(stream::iter(vec![Ok(chunk)]))
    }
}

pub(crate) fn create_stream_body_from_chunks(builder: &HttpBodyBuilder, chunks: &[&[u8]], options: &HttpBodyOptions) -> crate::HttpBody {
    let items: Vec<Result<BytesView>> = chunks
        .iter()
        .map(|chunk| Ok(BytesView::copied_from_slice(chunk, builder)))
        .collect();
    builder.clone().with_options(*options).stream(stream::iter(items))
}

/// A minimal [`http_body::Body`] implementation that yields a single chunk of data.
pub(crate) struct SingleChunkBody(Option<BytesView>);

impl SingleChunkBody {
    pub(crate) fn new(data: BytesView) -> Self {
        Self(Some(data))
    }
}

impl http_body::Body for SingleChunkBody {
    type Data = BytesView;
    type Error = HttpError;

    fn poll_frame(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<std::result::Result<Frame<Self::Data>, Self::Error>>> {
        Poll::Ready(self.0.take().map(|data| Ok(Frame::data(data))))
    }
}
