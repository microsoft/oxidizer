// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Request-parts, state, and one-body extraction contracts.

use alloc::string::String;
use alloc::vec::Vec;
use core::convert::Infallible;
use core::fmt;
use core::marker::PhantomData;
use core::ops::Deref;
use core::pin::Pin;
use core::task::{Context, Poll};

use bytes::{Bytes, BytesMut};
use http::request::Parts;
use http::{Extensions, HeaderMap, Method, StatusCode, Uri, Version};
use http_body::Body as HttpBody;

use crate::response::{Body, IntoResponse, Response};

/// Extracts a value from immutable HTTP request metadata and shared state.
///
/// The request lifetime permits the extracted value to borrow directly from
/// the parts. Extractors are synchronous, and generated routers call them
/// directly without allocating or boxing an extractor future. Generated
/// handlers require the rejection type to be identical for every request
/// lifetime and require its response body to be `'static`, so a rejection
/// cannot retain request metadata.
pub trait FromRequestParts<'request, S: ?Sized>: Sized {
    /// The response-producing extraction failure.
    type Rejection: IntoResponse;

    /// Extracts a value without consuming the request body.
    ///
    /// # Errors
    ///
    /// Returns a typed rejection when the request cannot produce this value.
    fn from_request_parts(parts: &'request Parts, state: &S) -> Result<Self, Self::Rejection>;
}

/// Extracts the request body exactly once.
///
/// Implementations return a concrete future through return-position
/// `impl Trait`; neither the future nor the request body is required to be
/// [`Send`]. Generated routers await this future directly and turn every
/// rejection into a response through [`IntoResponse`].
pub trait FromRequestBody<S: ?Sized, B>: Sized {
    /// The response-producing extraction failure.
    type Rejection: IntoResponse;

    /// Consumes `body` and produces the marked handler argument.
    ///
    /// # Errors
    ///
    /// Returns a typed rejection when body extraction fails.
    fn from_request_body(parts: &Parts, body: B, state: &S) -> impl Future<Output = Result<Self, Self::Rejection>>;
}

/// Supplies one concrete request-body witness for fixed-state validation.
///
/// A custom body extractor used by `#[router(state = S)]` implements this
/// trait for its state `S` and rejection `R`, then chooses any `RequestBody`
/// for which its [`FromRequestBody`] implementation exists. The generated
/// private witness checks that implementation when the service is defined.
/// Actual route calls still check the extractor against the caller's real
/// request-body type.
///
/// Bare `#[router]` services do not require this trait because their shared
/// state remains intentionally generic.
///
/// ```
/// use routerama::route::{BodyStateWitness, FromRequestBody, RequestParts, StatusCode};
///
/// struct AppState;
/// struct Document(Vec<u8>);
///
/// impl FromRequestBody<AppState, Vec<u8>> for Document {
///     type Rejection = StatusCode;
///
///     fn from_request_body(
///         _parts: &RequestParts,
///         body: Vec<u8>,
///         _state: &AppState,
///     ) -> impl Future<Output = Result<Self, Self::Rejection>> {
///         core::future::ready(Ok(Self(body)))
///     }
/// }
///
/// impl BodyStateWitness<AppState, StatusCode> for Document {
///     type RequestBody = Vec<u8>;
/// }
/// ```
pub trait BodyStateWitness<S: ?Sized, R: IntoResponse>: Sized {
    /// A request-body type proving this extractor supports `S`.
    type RequestBody;
}

/// Compile-only body used by built-in [`BodyStateWitness`] implementations.
///
/// Generated validation names the requested transport error in this body but
/// never constructs or polls a value.
#[doc(hidden)]
#[derive(Debug)]
pub struct BodyStateWitnessBody<E, D = Bytes>(PhantomData<fn() -> (E, D)>);

impl<E, D> HttpBody for BodyStateWitnessBody<E, D>
where
    D: bytes::Buf,
{
    type Data = D;
    type Error = E;

    fn poll_frame(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
        let _ = (self, cx);
        Poll::Ready(None)
    }
}

/// Projects handler state from the shared router state.
pub trait FromRef<T: ?Sized> {
    /// Creates a projection from shared state.
    fn from_ref(input: &T) -> Self;
}

impl<T> FromRef<T> for T
where
    T: Clone,
{
    fn from_ref(input: &T) -> Self {
        input.clone()
    }
}

/// Explicit application state extracted from the separately supplied state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct State<T>(pub T);

impl<T> State<T> {
    /// Consumes the wrapper and returns its state value.
    #[must_use]
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> Deref for State<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<S, T> FromRequestParts<'_, S> for State<T>
where
    S: ?Sized,
    T: FromRef<S>,
{
    type Rejection = Infallible;

    fn from_request_parts(_parts: &Parts, state: &S) -> Result<Self, Self::Rejection> {
        Ok(Self(T::from_ref(state)))
    }
}

impl<S> FromRequestParts<'_, S> for Method
where
    S: ?Sized,
{
    type Rejection = Infallible;

    fn from_request_parts(parts: &Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(parts.method.clone())
    }
}

impl<'request, S> FromRequestParts<'request, S> for &'request Method
where
    S: ?Sized,
{
    type Rejection = Infallible;

    fn from_request_parts(parts: &'request Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(&parts.method)
    }
}

impl<S> FromRequestParts<'_, S> for Uri
where
    S: ?Sized,
{
    type Rejection = Infallible;

    fn from_request_parts(parts: &Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(parts.uri.clone())
    }
}

impl<'request, S> FromRequestParts<'request, S> for &'request Uri
where
    S: ?Sized,
{
    type Rejection = Infallible;

    fn from_request_parts(parts: &'request Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(&parts.uri)
    }
}

impl<S> FromRequestParts<'_, S> for Version
where
    S: ?Sized,
{
    type Rejection = Infallible;

    fn from_request_parts(parts: &Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(parts.version)
    }
}

impl<'request, S> FromRequestParts<'request, S> for &'request Version
where
    S: ?Sized,
{
    type Rejection = Infallible;

    fn from_request_parts(parts: &'request Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(&parts.version)
    }
}

impl<S> FromRequestParts<'_, S> for HeaderMap
where
    S: ?Sized,
{
    type Rejection = Infallible;

    fn from_request_parts(parts: &Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(parts.headers.clone())
    }
}

impl<'request, S> FromRequestParts<'request, S> for &'request HeaderMap
where
    S: ?Sized,
{
    type Rejection = Infallible;

    fn from_request_parts(parts: &'request Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(&parts.headers)
    }
}

impl<'request, S> FromRequestParts<'request, S> for &'request Extensions
where
    S: ?Sized,
{
    type Rejection = Infallible;

    fn from_request_parts(parts: &'request Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(&parts.extensions)
    }
}

impl<'request, S> FromRequestParts<'request, S> for &'request Parts
where
    S: ?Sized,
{
    type Rejection = Infallible;

    fn from_request_parts(parts: &'request Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(parts)
    }
}

/// A zero-copy typed reference borrowed from request extensions.
#[derive(Clone, Copy, Debug)]
pub struct ExtensionRef<'request, T>(pub &'request T);

impl<'request, T> ExtensionRef<'request, T> {
    /// Returns the borrowed extension value.
    #[must_use]
    pub const fn get(&self) -> &'request T {
        self.0
    }

    /// Consumes the wrapper and returns the borrowed extension value.
    #[must_use]
    pub const fn into_inner(self) -> &'request T {
        self.0
    }
}

impl<T> Deref for ExtensionRef<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.0
    }
}

impl<'request, S, T> FromRequestParts<'request, S> for ExtensionRef<'request, T>
where
    S: ?Sized,
    T: Send + Sync + 'static,
{
    type Rejection = MissingExtension<T>;

    fn from_request_parts(parts: &'request Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts.extensions.get::<T>().map(Self).ok_or_else(MissingExtension::new)
    }
}

/// An explicitly cloned value from request extensions.
///
/// Prefer [`ExtensionRef`] unless the handler needs ownership beyond its
/// request-parts borrow.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ClonedExtension<T>(pub T);

impl<T> ClonedExtension<T> {
    /// Consumes the wrapper and returns the cloned extension value.
    #[must_use]
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> Deref for ClonedExtension<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<S, T> FromRequestParts<'_, S> for ClonedExtension<T>
where
    S: ?Sized,
    T: Clone + Send + Sync + 'static,
{
    type Rejection = MissingExtension<T>;

    fn from_request_parts(parts: &Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts.extensions.get::<T>().cloned().map(Self).ok_or_else(MissingExtension::new)
    }
}

/// A requested typed extension was absent.
///
/// Missing application-provided extensions indicate server configuration
/// failure and therefore become `500 Internal Server Error`.
pub struct MissingExtension<T> {
    marker: PhantomData<fn() -> T>,
}

impl<T> MissingExtension<T> {
    /// Creates a missing-extension rejection for `T`.
    #[must_use]
    pub const fn new() -> Self {
        Self { marker: PhantomData }
    }
}

impl<T> Default for MissingExtension<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> fmt::Debug for MissingExtension<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("MissingExtension").field(&core::any::type_name::<T>()).finish()
    }
}

impl<T> fmt::Display for MissingExtension<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "request extension `{}` is missing", core::any::type_name::<T>())
    }
}

impl<T> core::error::Error for MissingExtension<T> {}

impl<T> IntoResponse for MissingExtension<T> {
    type Body = Body;

    fn into_response(self) -> Response<Self::Body> {
        StatusCode::INTERNAL_SERVER_ERROR.into_response()
    }
}

/// Explicit ownership of an unmodified request body.
///
/// `RawBody<B>` performs no buffering and imposes no [`HttpBody`] bound. It is
/// the streaming escape hatch for handlers that need the transport's original
/// body type.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RawBody<B>(pub B);

impl<B> RawBody<B> {
    /// Consumes the wrapper and returns the original request body.
    #[must_use]
    pub fn into_inner(self) -> B {
        self.0
    }
}

impl<B> Deref for RawBody<B> {
    type Target = B;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<S, B> FromRequestBody<S, B> for RawBody<B>
where
    S: ?Sized,
{
    type Rejection = Infallible;

    fn from_request_body(_parts: &Parts, body: B, _state: &S) -> impl Future<Output = Result<Self, Self::Rejection>> {
        core::future::ready(Ok(Self(body)))
    }
}

impl<S, B> BodyStateWitness<S, Infallible> for RawBody<B>
where
    S: ?Sized,
{
    type RequestBody = B;
}

/// A request body buffered into bytes with an explicit maximum size.
///
/// `LIMIT` is the greatest number of data bytes accepted. An exact-size body
/// succeeds; the first frame that would cross the limit is rejected before it
/// is copied into the output buffer. A size-hint lower bound already above the
/// limit is rejected before polling, while every yielded frame remains checked
/// even when the hint is absent or incorrect.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BytesBody<const LIMIT: usize>(pub Bytes);

impl<const LIMIT: usize> BytesBody<LIMIT> {
    /// Returns the buffered bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Consumes the wrapper and returns the buffered bytes.
    #[must_use]
    pub fn into_inner(self) -> Bytes {
        self.0
    }
}

impl<const LIMIT: usize> Deref for BytesBody<LIMIT> {
    type Target = Bytes;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<S, B, const LIMIT: usize> FromRequestBody<S, B> for BytesBody<LIMIT>
where
    S: ?Sized,
    B: HttpBody<Data = Bytes>,
{
    type Rejection = BodyRejection<B::Error>;

    #[expect(
        clippy::manual_async_fn,
        reason = "the explicit future keeps unused parts and state references out of the generated state machine"
    )]
    fn from_request_body(_parts: &Parts, body: B, _state: &S) -> impl Future<Output = Result<Self, Self::Rejection>> {
        async move { collect_body::<B, LIMIT>(body).await.map(Self) }
    }
}

impl<S, E, const LIMIT: usize> BodyStateWitness<S, BodyRejection<E>> for BytesBody<LIMIT>
where
    S: ?Sized,
{
    type RequestBody = BodyStateWitnessBody<E>;
}

/// A request body buffered as UTF-8 text with an explicit maximum size.
///
/// The byte limit is enforced before UTF-8 validation. Invalid UTF-8 is
/// rejected with [`BodyRejection::InvalidUtf8`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TextBody<const LIMIT: usize>(pub String);

impl<const LIMIT: usize> TextBody<LIMIT> {
    /// Returns the buffered text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consumes the wrapper and returns the buffered text.
    #[must_use]
    pub fn into_inner(self) -> String {
        self.0
    }
}

impl<const LIMIT: usize> Deref for TextBody<LIMIT> {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<S, B, const LIMIT: usize> FromRequestBody<S, B> for TextBody<LIMIT>
where
    S: ?Sized,
    B: HttpBody<Data = Bytes>,
{
    type Rejection = BodyRejection<B::Error>;

    #[expect(
        clippy::manual_async_fn,
        reason = "the explicit future keeps unused parts and state references out of the generated state machine"
    )]
    fn from_request_body(_parts: &Parts, body: B, _state: &S) -> impl Future<Output = Result<Self, Self::Rejection>> {
        async move {
            let bytes = collect_body::<B, LIMIT>(body).await?;
            let text = String::from_utf8(Vec::from(bytes))
                .map_err(|error| BodyRejection::InvalidUtf8(InvalidUtf8Error::new(error.utf8_error())))?;
            Ok(Self(text))
        }
    }
}

impl<S, E, const LIMIT: usize> BodyStateWitness<S, BodyRejection<E>> for TextBody<LIMIT>
where
    S: ?Sized,
{
    type RequestBody = BodyStateWitnessBody<E>;
}

/// A request body buffered as validated UTF-8 while retaining its bytes.
///
/// Like [`TextBody`], the byte limit is enforced before UTF-8 validation and
/// invalid UTF-8 is rejected with [`BodyRejection::InvalidUtf8`]. Unlike
/// `TextBody`, this extractor keeps the collector's [`Bytes`]: a valid
/// single-frame body is not copied, while split frames use only the collector's
/// combined buffer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Utf8Body<const LIMIT: usize>(Bytes);

impl<const LIMIT: usize> Utf8Body<LIMIT> {
    /// Returns the validated text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        // SAFETY: construction is private, and extraction validates the complete
        // buffer before wrapping it. `Default` creates an empty, valid buffer.
        unsafe { core::str::from_utf8_unchecked(&self.0) }
    }

    /// Consumes the wrapper and returns the validated bytes.
    #[must_use]
    pub fn into_inner(self) -> Bytes {
        self.0
    }
}

impl<const LIMIT: usize> Deref for Utf8Body<LIMIT> {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

impl<S, B, const LIMIT: usize> FromRequestBody<S, B> for Utf8Body<LIMIT>
where
    S: ?Sized,
    B: HttpBody<Data = Bytes>,
{
    type Rejection = BodyRejection<B::Error>;

    #[expect(
        clippy::manual_async_fn,
        reason = "the explicit future keeps unused parts and state references out of the generated state machine"
    )]
    fn from_request_body(_parts: &Parts, body: B, _state: &S) -> impl Future<Output = Result<Self, Self::Rejection>> {
        async move {
            let bytes = collect_body::<B, LIMIT>(body).await?;
            core::str::from_utf8(&bytes).map_err(|error| BodyRejection::InvalidUtf8(InvalidUtf8Error::new(error)))?;
            Ok(Self(bytes))
        }
    }
}

impl<S, E, const LIMIT: usize> BodyStateWitness<S, BodyRejection<E>> for Utf8Body<LIMIT>
where
    S: ?Sized,
{
    type RequestBody = BodyStateWitnessBody<E>;
}

/// A request body exceeded its extractor's explicit byte limit.
///
/// `received` is the minimum size established at rejection: either data bytes
/// already yielded or the body's declared size-hint lower bound.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BodySizeLimitError {
    limit: usize,
    received: usize,
}

impl BodySizeLimitError {
    /// Creates a size-limit error from the configured limit and minimum size.
    #[must_use]
    pub const fn new(limit: usize, received: usize) -> Self {
        Self { limit, received }
    }

    /// Returns the configured maximum body size in bytes.
    #[must_use]
    pub const fn limit(&self) -> usize {
        self.limit
    }

    /// Returns the minimum body size established when the limit was crossed.
    #[must_use]
    pub const fn received(&self) -> usize {
        self.received
    }
}

impl fmt::Display for BodySizeLimitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "request body size {} bytes exceeds the explicit {} byte limit",
            self.received, self.limit
        )
    }
}

impl core::error::Error for BodySizeLimitError {}

/// A request body was delivered in more frames than its extractor's byte limit allows.
///
/// The frame budget is derived from the extractor's byte limit: every frame
/// must carry a useful average payload, so a body that arrives as a very large
/// number of tiny or empty frames is refused before its per-frame bookkeeping
/// outgrows the payload it describes. The budget is `limit / 64 + 64`, which
/// asks every frame to carry 64 bytes on average while leaving 64 frames of
/// slack for trailers and for limits too small to derive a useful budget from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BodyFrameLimitError {
    limit: usize,
    received: usize,
}

impl BodyFrameLimitError {
    /// Creates a frame-limit error from the frame budget and the frame count that crossed it.
    #[must_use]
    pub const fn new(limit: usize, received: usize) -> Self {
        Self { limit, received }
    }

    /// Returns the frame budget that was exceeded.
    #[must_use]
    pub const fn limit(&self) -> usize {
        self.limit
    }

    /// Returns the number of frames established when the budget was crossed.
    #[must_use]
    pub const fn received(&self) -> usize {
        self.received
    }
}

impl fmt::Display for BodyFrameLimitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "request body frame count {} exceeds the {} frame budget",
            self.received, self.limit
        )
    }
}

impl core::error::Error for BodyFrameLimitError {}

/// The smallest average payload a body frame must carry to be worth its bookkeeping.
const MIN_AVERAGE_FRAME_BYTES: usize = 64;

/// Frames allowed on top of the byte-derived budget, so that a small limit still
/// tolerates trailers and the occasional empty frame.
const FRAME_BUDGET_SLACK: usize = 64;

/// Returns the number of frames an extractor with `limit` bytes will read.
///
/// A frame that carries no data — an empty data frame, or a trailers frame —
/// charges nothing against the byte limit, and `poll_frame` may return `Ready`
/// indefinitely, so without a frame budget a body that supplies unlimited empty
/// frames never yields back to the executor. The budget also bounds per-frame
/// bookkeeping: a collector that retains one span per frame would otherwise let
/// a client choose an arbitrary multiple of the payload as overhead.
///
/// The budget is `limit / 64 + 64`, which asks every frame to carry 64 bytes on
/// average while leaving 64 frames of slack for trailers and for limits too
/// small to derive a useful budget from.
pub(super) const fn frame_budget(limit: usize) -> usize {
    (limit / MIN_AVERAGE_FRAME_BYTES).saturating_add(FRAME_BUDGET_SLACK)
}

/// The largest multiple of the bytes actually received that a size hint may reserve.
///
/// [`HttpBody::size_hint`] is verbatim client input for a `Content-Length`-framed
/// body, so reserving it outright lets a client that advertises a large body and
/// then sends almost nothing hold the whole limit resident. Capping the
/// reservation at a multiple of the delivered bytes keeps the hint's benefit for
/// an honest client while forcing an attacker to pay proportionally for the
/// memory it pins.
const MAX_HINT_AMPLIFICATION: usize = 16;

/// Returns the capacity to reserve for `received` delivered bytes given `hint`.
///
/// The result is never below `received`: a body may understate its size hint,
/// and the bytes already in hand must still fit.
const fn hinted_capacity(hint: usize, received: usize) -> usize {
    let earned = match received.checked_mul(MAX_HINT_AMPLIFICATION) {
        Some(earned) => earned,
        None => usize::MAX,
    };

    let trusted = if hint < earned { hint } else { earned };

    if trusted < received { received } else { trusted }
}

/// A request body's transport failed while yielding a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BodyTransportError<E> {
    error: E,
}

impl<E> BodyTransportError<E> {
    /// Wraps a transport error.
    #[must_use]
    pub const fn new(error: E) -> Self {
        Self { error }
    }

    /// Returns the transport error without erasing its concrete type.
    #[must_use]
    pub const fn error(&self) -> &E {
        &self.error
    }

    /// Consumes the wrapper and returns the transport error.
    #[must_use]
    pub fn into_inner(self) -> E {
        self.error
    }
}

impl<E> fmt::Display for BodyTransportError<E>
where
    E: fmt::Display,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "request body transport failed: {}", self.error)
    }
}

impl<E> core::error::Error for BodyTransportError<E>
where
    E: core::error::Error + 'static,
{
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        Some(&self.error)
    }
}

/// A request body was not valid UTF-8.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidUtf8Error {
    valid_up_to: usize,
    error_len: Option<usize>,
}

impl InvalidUtf8Error {
    /// Creates a body UTF-8 error without erasing its position details.
    #[must_use]
    pub fn new(error: core::str::Utf8Error) -> Self {
        Self {
            valid_up_to: error.valid_up_to(),
            error_len: error.error_len(),
        }
    }

    #[cfg(feature = "bytesbuf")]
    pub(super) const fn from_parts(valid_up_to: usize, error_len: Option<usize>) -> Self {
        Self { valid_up_to, error_len }
    }

    /// Returns the index through which the request body was valid UTF-8.
    #[must_use]
    pub const fn valid_up_to(&self) -> usize {
        self.valid_up_to
    }

    /// Returns the length of the invalid byte sequence, if it was complete.
    #[must_use]
    pub const fn error_len(&self) -> Option<usize> {
        self.error_len
    }
}

impl fmt::Display for InvalidUtf8Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "request body is not valid UTF-8 after byte {}", self.valid_up_to)
    }
}

impl core::error::Error for InvalidUtf8Error {}

/// A rejection produced while buffering or decoding a request body.
///
/// - [`TooLarge`](Self::TooLarge) becomes `413 Payload Too Large`;
/// - [`Transport`](Self::Transport) becomes `400 Bad Request`; and
/// - [`InvalidUtf8`](Self::InvalidUtf8) becomes `400 Bad Request`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BodyRejection<E> {
    /// The explicit byte limit was exceeded.
    TooLarge(BodySizeLimitError),
    /// The body was delivered in more frames than its byte limit allows.
    TooManyFrames(BodyFrameLimitError),
    /// The body transport failed while producing a frame.
    Transport(BodyTransportError<E>),
    /// Buffered bytes were not valid UTF-8.
    InvalidUtf8(InvalidUtf8Error),
}

impl<E> fmt::Display for BodyRejection<E>
where
    E: fmt::Display,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge(error) => error.fmt(f),
            Self::TooManyFrames(error) => error.fmt(f),
            Self::Transport(error) => error.fmt(f),
            Self::InvalidUtf8(error) => error.fmt(f),
        }
    }
}

impl<E> core::error::Error for BodyRejection<E>
where
    E: core::error::Error + 'static,
{
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::TooLarge(error) => Some(error),
            Self::TooManyFrames(error) => Some(error),
            Self::Transport(error) => Some(error),
            Self::InvalidUtf8(error) => Some(error),
        }
    }
}

impl<E> IntoResponse for BodyRejection<E> {
    type Body = Body;

    fn into_response(self) -> Response<Self::Body> {
        match self {
            Self::TooLarge(_) => StatusCode::PAYLOAD_TOO_LARGE,
            Self::TooManyFrames(_) | Self::Transport(_) | Self::InvalidUtf8(_) => StatusCode::BAD_REQUEST,
        }
        .into_response()
    }
}

pub(super) async fn collect_body<B, const LIMIT: usize>(body: B) -> Result<Bytes, BodyRejection<B::Error>>
where
    B: HttpBody<Data = Bytes>,
{
    let minimum = body.size_hint().lower();
    if minimum > u64::try_from(LIMIT).unwrap_or(u64::MAX) {
        let received = usize::try_from(minimum).unwrap_or(usize::MAX);
        return Err(BodyRejection::TooLarge(BodySizeLimitError::new(LIMIT, received)));
    }

    let mut body = core::pin::pin!(body);
    let mut first = None;
    let mut output = None;
    let hint = usize::try_from(minimum).unwrap_or(usize::MAX).min(LIMIT);
    let budget = frame_budget(LIMIT);
    let mut frames = 0_usize;

    while let Some(frame) = core::future::poll_fn(|context| body.as_mut().poll_frame(context)).await {
        frames = frames.saturating_add(1);
        if frames > budget {
            return Err(BodyRejection::TooManyFrames(BodyFrameLimitError::new(budget, frames)));
        }
        let frame = frame.map_err(|error| BodyRejection::Transport(BodyTransportError::new(error)))?;
        let Ok(data) = frame.into_data() else {
            continue;
        };
        if data.is_empty() {
            continue;
        }
        let received = output
            .as_ref()
            .map_or_else(|| first.as_ref().map_or(0, Bytes::len), BytesMut::len)
            .saturating_add(data.len());
        if received > LIMIT {
            return Err(BodyRejection::TooLarge(BodySizeLimitError::new(LIMIT, received)));
        }
        if let Some(output) = &mut output {
            output.extend_from_slice(&data);
        } else if let Some(initial) = first.take() {
            let mut combined = BytesMut::with_capacity(hinted_capacity(hint, received));
            combined.extend_from_slice(&initial);
            combined.extend_from_slice(&data);
            output = Some(combined);
        } else {
            first = Some(data);
        }
    }

    Ok(output.map_or_else(|| first.unwrap_or_default(), BytesMut::freeze))
}

/// A typed query-string value.
#[cfg(feature = "query")]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Query<T>(pub T);

#[cfg(feature = "query")]
impl<T> Query<T> {
    /// Consumes the wrapper and returns the parsed value.
    #[must_use]
    pub fn into_inner(self) -> T {
        self.0
    }
}

#[cfg(feature = "query")]
impl<T> Deref for Query<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// A query-string extraction failure.
#[cfg(feature = "query")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QueryRejection {
    error: crate::query::Error,
}

#[cfg(feature = "query")]
impl QueryRejection {
    /// Returns the query decoding error.
    #[must_use]
    pub const fn error(&self) -> &crate::query::Error {
        &self.error
    }
}

#[cfg(feature = "query")]
impl core::fmt::Display for QueryRejection {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        self.error.fmt(f)
    }
}

#[cfg(feature = "query")]
impl core::error::Error for QueryRejection {}

#[cfg(feature = "query")]
impl IntoResponse for QueryRejection {
    type Body = Body;

    fn into_response(self) -> Response<Self::Body> {
        http::StatusCode::BAD_REQUEST.into_response()
    }
}

#[cfg(feature = "query")]
impl<'request, S, T> FromRequestParts<'request, S> for Query<T>
where
    S: ?Sized,
    T: crate::query::FromQuery<'request>,
{
    type Rejection = QueryRejection;

    fn from_request_parts(parts: &'request Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let query = parts.uri.query().unwrap_or_default();
        T::from_query(query).map(Self).map_err(|error| QueryRejection { error })
    }
}
