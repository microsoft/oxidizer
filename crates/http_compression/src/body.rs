// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Adapts HTTP bodies for compression and decompression.
//!
//! `compressors` transforms a [`Stream`] of bytes. An HTTP body contains data
//! frames and trailers. The adapter stores trailers while it transforms data.
//!
//! [`CompressionChain`] represents all transformation stages as one concrete
//! type. After the data ends, [`CompressionStream::into_inner`] returns each
//! source stream. The chain unwraps these streams until it reaches the adapter
//! that stores the trailers. Each stage keeps its compression engine's concrete
//! type. The source uses [`StreamingBody`], which erases the body's concrete
//! type.
//!
//! The compression engine is a type parameter, so one chain supports both
//! compression and decompression.

use std::pin::Pin;
use std::task::{Context, Poll, ready};

use bytesbuf::BytesView;
use compressors::CompressionStream;
use compressors::core::{Compress, Compression, Decompress};
use futures::Stream;
use http::HeaderMap;
use http_body::{Body, Frame, SizeHint};
use http_extensions::body::StreamingBody;
use http_extensions::{HttpError, Result};
use seatbelt::{Recovery as _, RecoveryInfo};

use crate::error::{LABEL_COMPRESSION_INVALID, LABEL_COMPRESSION_LIMIT_EXCEEDED};
use crate::{CONTENT_DIGEST_HEADER, REPR_DIGEST_HEADER};

/// A body being read as a stream of transformed bytes.
///
/// A boxed layer gives the recursive type a finite size.
///
/// Each transformation stage allocates one box. Data chunks do not allocate a
/// box.
#[derive(Debug)]
pub(crate) enum CompressionChain<C> {
    /// The source body and any trailers read from it.
    Source { body: StreamingBody, trailers: Option<HeaderMap> },

    /// One transformation stage around the previous stream.
    Layer(Box<CompressionStream<Self, C>>),
}

impl<C> CompressionChain<C> {
    /// Starts a chain that reads data frames from `body`.
    pub(crate) fn new(body: StreamingBody) -> Self {
        Self::Source { body, trailers: None }
    }

    /// Returns the trailers read from the body.
    fn into_trailers(self) -> Option<HeaderMap> {
        match self {
            Self::Source { trailers, .. } => trailers,
            Self::Layer(stream) => stream.into_inner().into_trailers(),
        }
    }
}

impl<C: Compression<Mode = Decompress>> CompressionChain<C> {
    /// Adds one decompression stage.
    pub(crate) fn decompress(self, decompressor: C) -> Self {
        Self::Layer(Box::new(CompressionStream::decompress(self, decompressor)))
    }
}

impl<C: Compression<Mode = Compress>> CompressionChain<C> {
    /// Adds one compression stage.
    pub(crate) fn compress(self, compressor: C) -> Self {
        Self::Layer(Box::new(CompressionStream::compress(self, compressor).flush_on_pending(true)))
    }
}

impl<C: Compression> Stream for CompressionChain<C> {
    type Item = Result<BytesView>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.get_mut() {
            // Every layer is `Unpin` because it contains only a box.
            Self::Layer(stream) => Pin::new(stream.as_mut())
                .poll_next(cx)
                .map(|item| item.map(|chunk| chunk.map_err(to_http_error))),

            Self::Source { body, trailers } => loop {
                let Some(frame) = ready!(Pin::new(&mut *body).poll_frame(cx)) else {
                    return Poll::Ready(None);
                };

                match frame?.into_data() {
                    Ok(data) => return Poll::Ready(Some(Ok(data))),
                    // Store trailers and continue to the next frame.
                    Err(frame) => {
                        let new = frame.into_trailers().expect("a non-data HTTP frame must contain trailers");
                        match trailers.as_mut() {
                            Some(existing) => existing.extend(new),
                            None => *trailers = Some(new),
                        }
                    }
                }
            },
        }
    }
}

/// A body that is transformed as it is polled.
///
/// The transformation is lazy. A malformed or oversized body fails when this
/// body is read, not when its headers arrive.
///
/// The body drops a completed chain immediately. This releases its engines and
/// buffers before the caller drops the body.
#[derive(Debug)]
pub(crate) struct CompressionBody<C> {
    chain: Option<CompressionChain<C>>,
}

impl<C> CompressionBody<C> {
    pub(crate) const fn new(chain: CompressionChain<C>) -> Self {
        Self { chain: Some(chain) }
    }
}

impl<C: Compression> Body for CompressionBody<C> {
    type Data = BytesView;
    type Error = HttpError;

    fn poll_frame(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Self::Data>>>> {
        let this = self.get_mut();

        let Some(chain) = this.chain.as_mut() else {
            return Poll::Ready(None);
        };

        match ready!(Pin::new(chain).poll_next(cx)) {
            Some(Ok(data)) => Poll::Ready(Some(Ok(Frame::data(data)))),

            // The body failed, so its trailers must not be delivered.
            Some(Err(err)) => {
                this.chain = None;
                Poll::Ready(Some(Err(err)))
            }

            None => {
                let trailers = this.chain.take().and_then(CompressionChain::into_trailers).map(|mut trailers| {
                    trailers.remove(CONTENT_DIGEST_HEADER);
                    trailers.remove(REPR_DIGEST_HEADER);
                    trailers
                });

                Poll::Ready(trailers.map(|trailers| Ok(Frame::trailers(trailers))))
            }
        }
    }

    fn is_end_stream(&self) -> bool {
        self.chain.is_none()
    }

    /// The transformed length is unknown until the complete body is read.
    fn size_hint(&self) -> SizeHint {
        SizeHint::default()
    }
}

/// Converts an engine failure to an [`HttpError`].
pub(crate) fn to_http_error(err: compressors::Error) -> HttpError {
    if err.is_source() {
        // Preserve an `HttpError` from the source layer.
        if let Some(original) = err.into_source().and_then(|source| source.downcast::<HttpError>().ok()) {
            return *original;
        }

        return HttpError::other(
            "body compression or decompression failed",
            RecoveryInfo::never(),
            LABEL_COMPRESSION_INVALID,
        );
    }

    let label = if err.is_limit_exceeded() {
        LABEL_COMPRESSION_LIMIT_EXCEEDED
    } else {
        LABEL_COMPRESSION_INVALID
    };
    let recovery = err.recovery();

    HttpError::other(err, recovery, label)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use ohno::Labeled as _;

    use super::*;

    #[test]
    fn a_foreign_source_error_uses_the_generic_compression_failure() {
        let error = to_http_error(compressors::Error::other(
            "source failed",
            std::io::Error::other("not an HTTP error"),
        ));

        assert_eq!(error.label(), "compression_invalid");
        assert!(error.to_string().contains("body compression or decompression failed"));
    }
}
