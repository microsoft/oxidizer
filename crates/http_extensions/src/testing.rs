// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use bytesbuf::BytesView;
use futures::stream;

use crate::{HttpBodyBuilder, Result};

pub(crate) fn create_stream_body(builder: &HttpBodyBuilder, body: impl AsRef<[u8]>) -> crate::HttpBody {
    let data = body.as_ref();
    if data.is_empty() {
        builder.stream(stream::iter(Vec::<Result<BytesView>>::new()))
    } else {
        let chunk = BytesView::copied_from_slice(data, builder);
        builder.stream(stream::iter(vec![Ok(chunk)]))
    }
}

pub(crate) fn create_stream_body_from_chunks(builder: &HttpBodyBuilder, chunks: &[&[u8]]) -> crate::HttpBody {
    let items: Vec<Result<BytesView>> = chunks
        .iter()
        .map(|chunk| Ok(BytesView::copied_from_slice(chunk, builder)))
        .collect();
    builder.stream(stream::iter(items))
}
