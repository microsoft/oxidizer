// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Body plumbing shared by compression and decompression.
//!
//! `compressors` transforms a [`Stream`] of bytes, while an HTTP body is a
//! sequence of frames that also carries trailers. Bridging the two leaves the
//! trailers with nowhere to live for the duration of the transform.
//!
//! [`CompressionChain`] keeps them where they were read. It is recursive, so any
//! number of successive compression steps is still one concrete type. Once the
//! bytes run out, it is unwrapped through [`CompressionStream::into_inner`] back
//! to the adapter holding the trailers. Nothing is shared, and nothing is
//! type-erased.
//!
//! The same shape serves both directions: the engine is the only thing that
//! differs, so it is a type parameter.

use std::pin::Pin;
use std::task::{Context, Poll, ready};

use bytesbuf::BytesView;
use compressors::CompressionStream;
use compressors::core::{Compress, Compression, Decompress};
use futures::Stream;
use http::HeaderMap;
use http_body::{Body, Frame, SizeHint};
use http_extensions::{HttpBody, HttpError, Result};
use seatbelt::{Recovery as _, RecoveryInfo};

use crate::error::{LABEL_COMPRESSION_INVALID, LABEL_COMPRESSION_LIMIT_EXCEEDED};

/// A body being read as a stream of transformed bytes.
///
/// Boxing the layer is what gives the recursion a finite size; it costs one
/// allocation per compression or decompression stage, not per chunk.
#[derive(Debug)]
pub(crate) enum CompressionChain<C> {
    /// The bottom of the chain: the body itself, plus any trailers seen so far.
    Source {
        body: Pin<Box<HttpBody>>,
        trailers: Option<HeaderMap>,
    },

    /// One compression or decompression stage wrapping the preceding stream.
    Layer(Box<CompressionStream<Self, C>>),
}

impl<C> CompressionChain<C> {
    /// Starts a chain by reading data frames off `body`.
    pub(crate) fn new(body: HttpBody) -> Self {
        Self::Source {
            body: Box::pin(body),
            trailers: None,
        }
    }

    /// Unwraps the chain to recover the trailers read off the body.
    fn into_trailers(self) -> Option<HeaderMap> {
        match self {
            Self::Source { trailers, .. } => trailers,
            Self::Layer(stream) => stream.into_inner().into_trailers(),
        }
    }
}

impl<C: Compression<Mode = Decompress>> CompressionChain<C> {
    /// Adds one decompression stage on top of this one.
    pub(crate) fn decompress(self, decompressor: C) -> Self {
        Self::Layer(Box::new(CompressionStream::decompress(self, decompressor)))
    }
}

impl<C: Compression<Mode = Compress>> CompressionChain<C> {
    /// Adds one compression stage on top of this one.
    pub(crate) fn compress(self, compressor: C) -> Self {
        Self::Layer(Box::new(CompressionStream::compress(self, compressor).flush_on_pending(true)))
    }
}

impl<C: Compression> Stream for CompressionChain<C> {
    type Item = Result<BytesView>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.get_mut() {
            // Every layer is `Unpin`, because the only thing under it is a box.
            Self::Layer(stream) => Pin::new(stream.as_mut())
                .poll_next(cx)
                .map(|item| item.map(|chunk| chunk.map_err(to_http_error))),

            Self::Source { body, trailers } => loop {
                let Some(frame) = ready!(body.as_mut().poll_frame(cx)) else {
                    return Poll::Ready(None);
                };

                match frame?.into_data() {
                    Ok(data) => return Poll::Ready(Some(Ok(data))),
                    // Not a data frame; keep any trailers and read on.
                    Err(frame) => {
                        if let Ok(new) = frame.into_trailers() {
                            match trailers.as_mut() {
                                Some(existing) => existing.extend(new),
                                None => *trailers = Some(new),
                            }
                        }
                    }
                }
            },
        }
    }
}

/// A body that is transformed as it is polled.
///
/// The work is lazy, so a malformed or oversized body fails here rather than
/// when the headers arrive.
///
/// The chain is dropped as soon as it is finished with, which releases the
/// engines and their buffers without waiting for the caller to drop the body.
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

            // Trailers describe a body that was never delivered, so dropping the
            // chain here drops them too.
            Some(Err(err)) => {
                this.chain = None;
                Poll::Ready(Some(Err(err)))
            }

            None => {
                let trailers = this.chain.take().and_then(CompressionChain::into_trailers);

                Poll::Ready(trailers.map(|trailers| Ok(Frame::trailers(trailers))))
            }
        }
    }

    fn is_end_stream(&self) -> bool {
        self.chain.is_none()
    }

    /// The transformed length is not known until the body has been read.
    fn size_hint(&self) -> SizeHint {
        SizeHint::default()
    }
}

/// Translates an engine failure into an [`HttpError`].
fn to_http_error(err: compressors::Error) -> HttpError {
    if err.is_source() {
        // The source is the layer beneath, which already produced an
        // `HttpError`; take it back out rather than bury it under this one.
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
