// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::convert::Infallible;
use core::fmt;
use core::pin::Pin;
use core::task::{Context, Poll};

use bytes::Bytes;
use http_body::{Frame, SizeHint};
use pin_project_lite::pin_project;

/// Routerama's fixed in-memory response body.
///
/// `Body` stores at most one in-memory byte buffer and backs built-in response
/// conversions such as strings, bytes, and status codes. Generated routers can
/// combine it with arbitrary concrete streaming bodies in their service sum.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Body(Option<Bytes>);

impl Body {
    /// Creates an empty body.
    #[must_use]
    pub const fn empty() -> Self {
        Self(None)
    }

    /// Creates a body from an owned byte buffer.
    #[must_use]
    pub fn from_bytes(bytes: Bytes) -> Self {
        if bytes.is_empty() { Self::empty() } else { Self(Some(bytes)) }
    }

    /// Returns the body bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_deref().unwrap_or_default()
    }

    /// Consumes the body and returns its bytes.
    #[must_use]
    pub fn into_bytes(self) -> Bytes {
        self.0.unwrap_or_default()
    }
}

impl From<Bytes> for Body {
    fn from(bytes: Bytes) -> Self {
        Self::from_bytes(bytes)
    }
}

impl From<()> for Body {
    fn from((): ()) -> Self {
        Self::empty()
    }
}

impl From<Vec<u8>> for Body {
    fn from(bytes: Vec<u8>) -> Self {
        Self::from_bytes(Bytes::from(bytes))
    }
}

impl From<String> for Body {
    fn from(text: String) -> Self {
        Self::from_bytes(Bytes::from(text))
    }
}

impl From<&str> for Body {
    fn from(text: &str) -> Self {
        Self::from_bytes(Bytes::copy_from_slice(text.as_bytes()))
    }
}

impl From<&[u8]> for Body {
    fn from(bytes: &[u8]) -> Self {
        Self::from_bytes(Bytes::copy_from_slice(bytes))
    }
}

impl http_body::Body for Body {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(mut self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        Poll::Ready(self.0.take().map(|bytes| Ok(Frame::data(bytes))))
    }

    fn is_end_stream(&self) -> bool {
        self.0.is_none()
    }

    fn size_hint(&self) -> SizeHint {
        let length = self.0.as_ref().map_or(0, |bytes| bytes.len() as u64);
        SizeHint::with_exact(length)
    }
}

/// An uninhabited response body for conversions that cannot fail.
///
/// This keeps impossible [`Infallible`] response branches from adding a
/// discriminant or storage to concrete body sums.
#[derive(Clone, Copy, Debug)]
pub enum NeverBody {}

impl http_body::Body for NeverBody {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let _ = self;
        unreachable!("NeverBody has no values and therefore cannot be polled")
    }

    fn is_end_stream(&self) -> bool {
        unreachable!("NeverBody has no values and therefore cannot be inspected")
    }

    fn size_hint(&self) -> SizeHint {
        unreachable!("NeverBody has no values and therefore cannot have a size hint")
    }
}

pin_project! {
    /// A concrete two-way response body used by [`Result`](core::result::Result)
    /// response conversion.
    ///
    /// Each branch retains its original body. Polling delegates directly to the
    /// active branch and maps only its error into [`EitherBodyError`].
    #[derive(Clone, Copy, Debug)]
    #[allow(missing_docs, reason = "the body fields are described by their public variants")]
    #[project = EitherBodyProj]
    pub enum EitherBody<L, R> {
        /// The successful or left branch.
        Left {
            #[pin]
            body: L,
        },
        /// The error or right branch.
        Right {
            #[pin]
            body: R,
        },
    }
}

/// An error from one branch of an [`EitherBody`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EitherBodyError<L, R> {
    /// The left body failed.
    Left(L),
    /// The right body failed.
    Right(R),
}

/// Frame data from one branch of a [`DataEitherBody`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EitherData<L, R> {
    /// Data yielded by the left body.
    Left(L),
    /// Data yielded by the right body.
    Right(R),
}

impl<L, R> bytes::Buf for EitherData<L, R>
where
    L: bytes::Buf,
    R: bytes::Buf,
{
    fn remaining(&self) -> usize {
        match self {
            Self::Left(data) => data.remaining(),
            Self::Right(data) => data.remaining(),
        }
    }

    fn chunk(&self) -> &[u8] {
        match self {
            Self::Left(data) => data.chunk(),
            Self::Right(data) => data.chunk(),
        }
    }

    #[cfg(feature = "bytesbuf-std")]
    fn chunks_vectored<'a>(&'a self, dst: &mut [std::io::IoSlice<'a>]) -> usize {
        match self {
            Self::Left(data) => data.chunks_vectored(dst),
            Self::Right(data) => data.chunks_vectored(dst),
        }
    }

    fn advance(&mut self, cnt: usize) {
        match self {
            Self::Left(data) => data.advance(cnt),
            Self::Right(data) => data.advance(cnt),
        }
    }
}

impl<L, R> fmt::Display for EitherBodyError<L, R>
where
    L: fmt::Display,
    R: fmt::Display,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Left(error) => error.fmt(f),
            Self::Right(error) => error.fmt(f),
        }
    }
}

impl<L, R> core::error::Error for EitherBodyError<L, R>
where
    L: core::error::Error + 'static,
    R: core::error::Error + 'static,
{
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::Left(error) => Some(error),
            Self::Right(error) => Some(error),
        }
    }
}

impl<L, R> http_body::Body for EitherBody<L, R>
where
    L: http_body::Body,
    R: http_body::Body<Data = L::Data>,
{
    type Data = L::Data;
    type Error = EitherBodyError<L::Error, R::Error>;

    fn poll_frame(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        match self.project() {
            EitherBodyProj::Left { body } => match body.poll_frame(cx) {
                Poll::Ready(Some(Ok(frame))) => Poll::Ready(Some(Ok(frame))),
                Poll::Ready(Some(Err(error))) => Poll::Ready(Some(Err(EitherBodyError::Left(error)))),
                Poll::Ready(None) => Poll::Ready(None),
                Poll::Pending => Poll::Pending,
            },
            EitherBodyProj::Right { body } => match body.poll_frame(cx) {
                Poll::Ready(Some(Ok(frame))) => Poll::Ready(Some(Ok(frame))),
                Poll::Ready(Some(Err(error))) => Poll::Ready(Some(Err(EitherBodyError::Right(error)))),
                Poll::Ready(None) => Poll::Ready(None),
                Poll::Pending => Poll::Pending,
            },
        }
    }

    fn is_end_stream(&self) -> bool {
        match self {
            Self::Left { body } => body.is_end_stream(),
            Self::Right { body } => body.is_end_stream(),
        }
    }

    fn size_hint(&self) -> SizeHint {
        match self {
            Self::Left { body } => body.size_hint(),
            Self::Right { body } => body.size_hint(),
        }
    }
}

pin_project! {
    /// A two-way body that preserves different frame-data types.
    ///
    /// Unlike [`EitherBody`], this explicit body maps successful data frames
    /// into [`EitherData`]. Trailers are forwarded unchanged.
    #[derive(Clone, Copy, Debug)]
    #[allow(missing_docs, reason = "the body fields are described by their public variants")]
    #[project = DataEitherBodyProj]
    pub enum DataEitherBody<L, R> {
        /// The left branch.
        Left {
            #[pin]
            body: L,
        },
        /// The right branch.
        Right {
            #[pin]
            body: R,
        },
    }
}

impl<L, R> http_body::Body for DataEitherBody<L, R>
where
    L: http_body::Body,
    R: http_body::Body,
{
    type Data = EitherData<L::Data, R::Data>;
    type Error = EitherBodyError<L::Error, R::Error>;

    fn poll_frame(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        match self.project() {
            DataEitherBodyProj::Left { body } => match body.poll_frame(cx) {
                Poll::Ready(Some(Ok(frame))) => Poll::Ready(Some(Ok(frame.map_data(EitherData::Left)))),
                Poll::Ready(Some(Err(error))) => Poll::Ready(Some(Err(EitherBodyError::Left(error)))),
                Poll::Ready(None) => Poll::Ready(None),
                Poll::Pending => Poll::Pending,
            },
            DataEitherBodyProj::Right { body } => match body.poll_frame(cx) {
                Poll::Ready(Some(Ok(frame))) => Poll::Ready(Some(Ok(frame.map_data(EitherData::Right)))),
                Poll::Ready(Some(Err(error))) => Poll::Ready(Some(Err(EitherBodyError::Right(error)))),
                Poll::Ready(None) => Poll::Ready(None),
                Poll::Pending => Poll::Pending,
            },
        }
    }

    fn is_end_stream(&self) -> bool {
        match self {
            Self::Left { body } => body.is_end_stream(),
            Self::Right { body } => body.is_end_stream(),
        }
    }

    fn size_hint(&self) -> SizeHint {
        match self {
            Self::Left { body } => body.size_hint(),
            Self::Right { body } => body.size_hint(),
        }
    }
}

/// The erased error produced by an explicit [`BoxBody`] boundary.
#[derive(Debug)]
pub struct BoxBodyError(Box<dyn core::error::Error + 'static>);

impl BoxBodyError {
    /// Returns the erased concrete body error.
    #[must_use]
    pub fn as_error(&self) -> &(dyn core::error::Error + 'static) {
        self.0.as_ref()
    }

    /// Consumes the wrapper and returns the boxed concrete body error.
    #[must_use]
    pub fn into_inner(self) -> Box<dyn core::error::Error + 'static> {
        self.0
    }
}

impl fmt::Display for BoxBodyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl core::error::Error for BoxBodyError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        Some(self.0.as_ref())
    }
}

pin_project! {
    struct EraseBodyError<B> {
        #[pin]
        body: B,
    }
}

impl<B> http_body::Body for EraseBodyError<B>
where
    B: http_body::Body,
    B::Error: core::error::Error + 'static,
{
    type Data = B::Data;
    type Error = BoxBodyError;

    fn poll_frame(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        match self.project().body.poll_frame(cx) {
            Poll::Ready(Some(Ok(frame))) => Poll::Ready(Some(Ok(frame))),
            Poll::Ready(Some(Err(error))) => Poll::Ready(Some(Err(BoxBodyError(Box::new(error))))),
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }

    fn is_end_stream(&self) -> bool {
        self.body.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.body.size_hint()
    }
}

/// The erased error produced by an explicit [`SendBoxBody`] boundary.
///
/// The inner error keeps `Send + Sync` so this value converts into the
/// `Box<dyn Error + Send + Sync>` that Hyper, Axum, and Tower middleware
/// expect.
#[derive(Debug)]
pub struct SendBoxBodyError(Box<dyn core::error::Error + Send + Sync + 'static>);

impl SendBoxBodyError {
    /// Returns the erased concrete body error.
    #[must_use]
    pub fn as_error(&self) -> &(dyn core::error::Error + Send + Sync + 'static) {
        self.0.as_ref()
    }

    /// Consumes the wrapper and returns the boxed concrete body error.
    #[must_use]
    pub fn into_inner(self) -> Box<dyn core::error::Error + Send + Sync + 'static> {
        self.0
    }
}

impl fmt::Display for SendBoxBodyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl core::error::Error for SendBoxBodyError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        Some(self.0.as_ref())
    }
}

pin_project! {
    struct EraseSendBodyError<B> {
        #[pin]
        body: B,
    }
}

impl<B> http_body::Body for EraseSendBodyError<B>
where
    B: http_body::Body,
    B::Error: core::error::Error + Send + Sync + 'static,
{
    type Data = B::Data;
    type Error = SendBoxBodyError;

    fn poll_frame(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        match self.project().body.poll_frame(cx) {
            Poll::Ready(Some(Ok(frame))) => Poll::Ready(Some(Ok(frame))),
            Poll::Ready(Some(Err(error))) => Poll::Ready(Some(Err(SendBoxBodyError(Box::new(error))))),
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }

    fn is_end_stream(&self) -> bool {
        self.body.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.body.size_hint()
    }
}

/// An explicitly type-erased [`Send`] response body.
///
/// This is [`BoxBody`]'s transport-facing companion. It exists because a
/// generated router's response body is a private concrete sum returned behind
/// an opaque type. A generated `#[router(..., tower)]` adapter can preserve
/// that exact body behind its own opaque service type; other adapters that
/// must *name* one open response type can erase it here. Erasure is the
/// caller's explicit choice, never an internal default.
///
/// Constructing a `SendBoxBody` allocates exactly once and every frame poll
/// uses dynamic dispatch. Data frames, trailers, [`size_hint`], and
/// [`is_end_stream`] are forwarded unchanged; body errors are boxed only if
/// they occur.
///
/// [`size_hint`]: http_body::Body::size_hint
/// [`is_end_stream`]: http_body::Body::is_end_stream
pub struct SendBoxBody<D: bytes::Buf = Bytes> {
    body: Pin<Box<dyn http_body::Body<Data = D, Error = SendBoxBodyError> + Send + 'static>>,
}

impl<D> SendBoxBody<D>
where
    D: bytes::Buf,
{
    /// Erases one concrete `Send` response body.
    ///
    /// The body must be `Send + 'static` and its error `Send + Sync + 'static`
    /// because both are stored behind owned trait objects that cross a
    /// transport boundary. Use [`BoxBody`] instead when the response stays on
    /// one thread.
    #[must_use]
    pub fn new<B>(body: B) -> Self
    where
        B: http_body::Body<Data = D> + Send + 'static,
        B::Error: core::error::Error + Send + Sync + 'static,
    {
        Self {
            body: Box::pin(EraseSendBodyError { body }),
        }
    }
}

impl<D> fmt::Debug for SendBoxBody<D>
where
    D: bytes::Buf,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SendBoxBody").finish_non_exhaustive()
    }
}

impl<D> http_body::Body for SendBoxBody<D>
where
    D: bytes::Buf,
{
    type Data = D;
    type Error = SendBoxBodyError;

    fn poll_frame(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        self.body.as_mut().poll_frame(cx)
    }

    fn is_end_stream(&self) -> bool {
        self.body.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.body.size_hint()
    }
}

/// An explicitly type-erased response body.
///
/// Constructing a `BoxBody` allocates once and every frame poll uses dynamic
/// dispatch. Body errors are boxed only if they occur. Generated routers never
/// construct this type unless a handler explicitly returns it.
///
/// This boundary is deliberately local: it imposes no [`Send`] bound, so a
/// response body holding an [`Rc`](alloc::rc::Rc) still passes through it. Use
/// [`SendBoxBody`] when a transport requires a `Send` response body.
pub struct BoxBody<D: bytes::Buf = Bytes> {
    body: Pin<Box<dyn http_body::Body<Data = D, Error = BoxBodyError> + 'static>>,
}

impl<D> BoxBody<D>
where
    D: bytes::Buf,
{
    /// Erases one concrete response body.
    ///
    /// This opt-in boundary requires a `'static` body and error because both
    /// are stored behind owned trait objects. It does not require [`Send`] or
    /// [`Sync`].
    #[must_use]
    pub fn new<B>(body: B) -> Self
    where
        B: http_body::Body<Data = D> + 'static,
        B::Error: core::error::Error + 'static,
    {
        Self {
            body: Box::pin(EraseBodyError { body }),
        }
    }
}

impl<D> fmt::Debug for BoxBody<D>
where
    D: bytes::Buf,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BoxBody").finish_non_exhaustive()
    }
}

impl<D> http_body::Body for BoxBody<D>
where
    D: bytes::Buf,
{
    type Data = D;
    type Error = BoxBodyError;

    fn poll_frame(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        self.body.as_mut().poll_frame(cx)
    }

    fn is_end_stream(&self) -> bool {
        self.body.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.body.size_hint()
    }
}
