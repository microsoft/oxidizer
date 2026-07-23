// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Bounded request extraction that preserves `bytesbuf::BytesView` storage.

use core::convert::Infallible;
use core::fmt;
use core::ops::Deref;

use bytesbuf::{BytesBuf, BytesView};
use http::request::Parts;
use http_body::Body as HttpBody;

use super::extract::BodyStateWitnessBody;
use super::{
    BodyFrameLimitError, BodyRejection, BodySizeLimitError, BodyStateWitness, BodyTransportError, FromRequestBody, InvalidUtf8Error,
};

/// A bounded request body collected without copying its payload bytes.
///
/// A single non-empty data frame is returned unchanged. Multiple views are
/// composed by transferring their existing spans into one logical view.
#[derive(Clone, Debug, Default)]
pub struct BytesViewBody<const LIMIT: usize>(pub BytesView);

impl<const LIMIT: usize> BytesViewBody<LIMIT> {
    /// Returns the collected view.
    #[must_use]
    pub const fn view(&self) -> &BytesView {
        &self.0
    }

    /// Consumes the wrapper and returns the collected view.
    #[must_use]
    pub fn into_inner(self) -> BytesView {
        self.0
    }
}

impl<const LIMIT: usize> Deref for BytesViewBody<LIMIT> {
    type Target = BytesView;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<S, B, const LIMIT: usize> FromRequestBody<S, B> for BytesViewBody<LIMIT>
where
    S: ?Sized,
    B: HttpBody<Data = BytesView>,
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

impl<S, const LIMIT: usize> BodyStateWitness<S, BodyRejection<Infallible>> for BytesViewBody<LIMIT>
where
    S: ?Sized,
{
    type RequestBody = BodyStateWitnessBody<Infallible, BytesView>;
}

/// A bounded, logically UTF-8 request body that retains its fragmented view.
///
/// Use [`view()`](Self::view) or [`BytesView::slices`] to consume it. This type
/// does not expose `Deref<Target = str>` because valid text may span multiple
/// non-contiguous slices.
#[derive(Clone, Debug, Default)]
pub struct Utf8BytesViewBody<const LIMIT: usize>(BytesView);

impl<const LIMIT: usize> Utf8BytesViewBody<LIMIT> {
    /// Returns the validated byte view.
    #[must_use]
    pub const fn view(&self) -> &BytesView {
        &self.0
    }

    /// Consumes the wrapper and returns the validated byte view.
    #[must_use]
    pub fn into_inner(self) -> BytesView {
        self.0
    }
}

impl<const LIMIT: usize> Deref for Utf8BytesViewBody<LIMIT> {
    type Target = BytesView;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<S, B, const LIMIT: usize> FromRequestBody<S, B> for Utf8BytesViewBody<LIMIT>
where
    S: ?Sized,
    B: HttpBody<Data = BytesView>,
{
    type Rejection = BodyRejection<B::Error>;

    #[expect(
        clippy::manual_async_fn,
        reason = "the explicit future keeps unused parts and state references out of the generated state machine"
    )]
    fn from_request_body(_parts: &Parts, body: B, _state: &S) -> impl Future<Output = Result<Self, Self::Rejection>> {
        async move {
            let view = collect_body::<B, LIMIT>(body).await?;
            validate_utf8(&view).map_err(BodyRejection::InvalidUtf8)?;
            Ok(Self(view))
        }
    }
}

impl<S, const LIMIT: usize> BodyStateWitness<S, BodyRejection<Infallible>> for Utf8BytesViewBody<LIMIT>
where
    S: ?Sized,
{
    type RequestBody = BodyStateWitnessBody<Infallible, BytesView>;
}

/// Owned JSON decoded directly from a fragmented [`BytesView`].
///
/// This extractor requires the additive `json` and `bytesbuf-std` features
/// because `serde_json` consumes the view through its `std::io::Read`
/// implementation.
#[cfg(all(feature = "json", feature = "bytesbuf-std"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JsonView<T, const LIMIT: usize>(pub T);

#[cfg(all(feature = "json", feature = "bytesbuf-std"))]
impl<T, const LIMIT: usize> JsonView<T, LIMIT> {
    /// Consumes the wrapper and returns the decoded value.
    #[must_use]
    pub fn into_inner(self) -> T {
        self.0
    }
}

#[cfg(all(feature = "json", feature = "bytesbuf-std"))]
impl<T, const LIMIT: usize> Deref for JsonView<T, LIMIT> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[cfg(all(feature = "json", feature = "bytesbuf-std"))]
impl<S, B, T, const LIMIT: usize> FromRequestBody<S, B> for JsonView<T, LIMIT>
where
    S: ?Sized,
    B: HttpBody<Data = BytesView>,
    T: serde::de::DeserializeOwned,
{
    type Rejection = super::json::JsonRejection<B::Error>;

    fn from_request_body(parts: &Parts, body: B, _state: &S) -> impl Future<Output = Result<Self, Self::Rejection>> {
        let content_type = super::json::validate_content_type(parts);
        async move {
            content_type.map_err(super::json::JsonRejection::UnsupportedMediaType)?;
            let view = collect_body::<B, LIMIT>(body).await.map_err(super::json::JsonRejection::Body)?;
            serde_json::from_reader(view)
                .map(Self)
                .map_err(|error| super::json::JsonRejection::Malformed(super::json::JsonDecodeError::new(error)))
        }
    }
}

#[cfg(all(feature = "json", feature = "bytesbuf-std"))]
impl<S, T, const LIMIT: usize> BodyStateWitness<S, super::json::JsonRejection<Infallible>> for JsonView<T, LIMIT>
where
    S: ?Sized,
{
    type RequestBody = BodyStateWitnessBody<Infallible, BytesView>;
}

async fn collect_body<B, const LIMIT: usize>(body: B) -> Result<BytesView, BodyRejection<B::Error>>
where
    B: HttpBody<Data = BytesView>,
{
    let minimum = body.size_hint().lower();
    if minimum > u64::try_from(LIMIT).unwrap_or(u64::MAX) {
        let received = usize::try_from(minimum).unwrap_or(usize::MAX);
        return Err(BodyRejection::TooLarge(BodySizeLimitError::new(LIMIT, received)));
    }

    let mut body = core::pin::pin!(body);
    let mut first = None;
    let mut combined: Option<BytesBuf> = None;
    let mut received = 0_usize;
    let budget = super::extract::frame_budget(LIMIT);
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
        received = received.saturating_add(data.len());
        if received > LIMIT {
            return Err(BodyRejection::TooLarge(BodySizeLimitError::new(LIMIT, received)));
        }
        if let Some(output) = &mut combined {
            output.put_bytes(data);
        } else if let Some(initial) = first.take() {
            let mut output = BytesBuf::new();
            output.put_bytes(initial);
            output.put_bytes(data);
            combined = Some(output);
        } else {
            first = Some(data);
        }
    }

    Ok(combined.map_or_else(|| first.unwrap_or_default(), |mut output| output.consume_all()))
}

fn validate_utf8(view: &BytesView) -> Result<(), InvalidUtf8Error> {
    let mut carry = [0_u8; 4];
    let mut carry_len = 0_usize;
    let mut carry_start = 0_usize;
    let mut absolute = 0_usize;

    for (slice, _) in view.slices() {
        let mut consumed = 0_usize;
        if carry_len != 0 {
            let expected = utf8_sequence_length(carry[0]);
            let take = (expected - carry_len).min(slice.len());
            carry[carry_len..carry_len + take].copy_from_slice(&slice[..take]);
            carry_len += take;
            consumed = take;
            if carry_len < expected {
                absolute = absolute.saturating_add(slice.len());
                continue;
            }
            if let Err(error) = core::str::from_utf8(&carry[..expected]) {
                return Err(InvalidUtf8Error::from_parts(
                    carry_start.saturating_add(error.valid_up_to()),
                    error.error_len(),
                ));
            }
            carry_len = 0;
        }

        let remaining = &slice[consumed..];
        if let Err(error) = core::str::from_utf8(remaining) {
            let valid_up_to = error.valid_up_to();
            let error_start = absolute.saturating_add(consumed).saturating_add(valid_up_to);
            if let Some(error_len) = error.error_len() {
                return Err(InvalidUtf8Error::from_parts(error_start, Some(error_len)));
            }
            let incomplete = &remaining[valid_up_to..];
            carry[..incomplete.len()].copy_from_slice(incomplete);
            carry_len = incomplete.len();
            carry_start = error_start;
        }
        absolute = absolute.saturating_add(slice.len());
    }

    if carry_len == 0 {
        Ok(())
    } else {
        let error = core::str::from_utf8(&carry[..carry_len]).expect_err("a retained UTF-8 suffix is incomplete");
        Err(InvalidUtf8Error::from_parts(
            carry_start.saturating_add(error.valid_up_to()),
            error.error_len(),
        ))
    }
}

const fn utf8_sequence_length(first: u8) -> usize {
    match first {
        0xc2..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf4 => 4,
        _ => 1,
    }
}

impl<const LIMIT: usize> fmt::Display for BytesViewBody<LIMIT> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} collected bytes", self.0.len())
    }
}
