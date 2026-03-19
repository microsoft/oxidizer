// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use bytesbuf::BytesView;
use futures::stream;
use http_body::Frame;
use http_body_util::StreamBody;

use crate::{HttpBodyBuilder, Result};

/// Creates an [`HttpBody`][crate::HttpBody] from raw bytes using
/// [`HttpBodyBuilder::stream`], simulating a streaming body without any real
/// network IO.
///
/// This is the recommended way to build bodies in tests that exercise the
/// streaming / external-body code paths.
pub fn create_stream_body(builder: &HttpBodyBuilder, body: impl AsRef<[u8]>) -> crate::HttpBody {
    let data = body.as_ref();
    if data.is_empty() {
        builder.stream(stream::iter(Vec::<Result<BytesView>>::new()))
    } else {
        let chunk = BytesView::copied_from_slice(data, builder);
        builder.stream(stream::iter(vec![Ok(chunk)]))
    }
}

/// Creates an [`HttpBody`][crate::HttpBody] from raw bytes using
/// [`HttpBodyBuilder::external`], wrapping a [`StreamBody`] directly.
///
/// This exercises the `Kind::Body` variant inside `HttpBody`.
pub fn create_external_body(builder: &HttpBodyBuilder, body: impl AsRef<[u8]>) -> crate::HttpBody {
    let data = body.as_ref();
    if data.is_empty() {
        let framed = stream::iter(Vec::<Result<Frame<BytesView>>>::new());
        builder.external(StreamBody::new(framed))
    } else {
        let chunk = BytesView::copied_from_slice(data, builder);
        let framed = stream::iter(vec![Ok::<_, crate::HttpError>(Frame::data(chunk))]);
        builder.external(StreamBody::new(framed))
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use futures::executor::block_on;

    use super::*;

    #[test]
    fn stream_body_with_data() {
        let builder = HttpBodyBuilder::new_fake();
        let body = create_stream_body(&builder, b"hello world");
        let text = block_on(body.into_text()).unwrap();
        assert_eq!(text, "hello world");
    }

    #[test]
    fn stream_body_empty() {
        let builder = HttpBodyBuilder::new_fake();
        let body = create_stream_body(&builder, b"");
        let bytes = block_on(body.into_bytes()).unwrap();
        assert!(bytes.is_empty());
    }

    #[test]
    fn external_body_with_data() {
        let builder = HttpBodyBuilder::new_fake();
        let body = create_external_body(&builder, b"external payload");
        let text = block_on(body.into_text()).unwrap();
        assert_eq!(text, "external payload");
    }

    #[test]
    fn external_body_empty() {
        let builder = HttpBodyBuilder::new_fake();
        let body = create_external_body(&builder, b"");
        let bytes = block_on(body.into_bytes()).unwrap();
        assert!(bytes.is_empty());
    }

    #[test]
    fn stream_body_integrates_with_http_body_builder() {
        let builder = HttpBodyBuilder::new_fake();
        let body = create_stream_body(&builder, b"integration test");
        let text = block_on(body.into_text()).unwrap();
        assert_eq!(text, "integration test");
    }
}
