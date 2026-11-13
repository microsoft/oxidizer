// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The media types whose bodies the layer logs.

use std::sync::Arc;

use http::HeaderMap;
use http::header::CONTENT_TYPE;
use http_headers::headers::{MediaRange, MediaRangeKind};

/// A configured media type is not valid.
#[ohno::error]
#[display("'{entry}' is not a media type")]
pub struct MediaTypeError {
    entry: String,
}

/// The media types whose bodies the layer logs.
#[derive(Clone, Default)]
pub(crate) struct MediaTypes(Arc<[MediaRange<'static>]>);

impl MediaTypes {
    pub(crate) fn parse(entries: impl IntoIterator<Item = &'static str>) -> Result<Self, MediaTypeError> {
        entries
            .into_iter()
            .map(|entry| {
                media_range(entry)
                    .filter(|range| range.kind() == MediaRangeKind::Exact)
                    .ok_or_else(|| MediaTypeError::new(entry))
            })
            .collect::<Result<_, MediaTypeError>>()
            .map(Self)
    }

    /// Whether the types cover the `Content-Type` of `headers`.
    pub(crate) fn matches(&self, headers: &HeaderMap) -> bool {
        if self.0.is_empty() {
            return false;
        }
        let Some(range) = headers.get(CONTENT_TYPE).and_then(|value| media_range(value.to_str().ok()?)) else {
            return false;
        };
        let (type_, subtype) = (range.type_().as_str(), range.subtype().as_str());
        self.0
            .iter()
            .any(|entry| entry.type_().eq_ignore_ascii_case(type_) && entry.subtype().eq_ignore_ascii_case(subtype))
            || type_.eq_ignore_ascii_case("text")
            || (type_.eq_ignore_ascii_case("application")
                && subtype
                    .rsplit_once('+')
                    .is_some_and(|(_, suffix)| suffix.eq_ignore_ascii_case("json") || suffix.eq_ignore_ascii_case("xml")))
    }
}

/// The type and the subtype of the media type `text`, without its parameters.
fn media_range(text: &str) -> Option<MediaRange<'_>> {
    let essence = text.split_once(';').map_or(text, |(essence, _)| essence);
    // #[gamma::skip(collection.reverse_array, tag = "equivalent", reason = "The characters to trim are a set, so their order is not observable.")]
    MediaRange::parse(essence.trim_matches([' ', '\t'])).ok()
}
