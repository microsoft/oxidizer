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
            .map(|entry| media_range(entry).ok_or_else(|| MediaTypeError::new(entry)))
            .collect::<Result<_, MediaTypeError>>()
            .map(Self)
    }

    /// Whether the `Content-Type` of `headers` matches one of the types.
    ///
    /// The rule is the one of `http_compression`: a `*` type or subtype is a wildcard, a subtype such as `json` also
    /// covers a structured suffix such as `ld+json`, and ASCII case and parameters are ignored. Only a whole `*` is a
    /// wildcard, so `*+json` is not one.
    pub(crate) fn matches(&self, headers: &HeaderMap) -> bool {
        if self.0.is_empty() {
            return false;
        }
        let Some(range) = headers.get(CONTENT_TYPE).and_then(|value| media_range(value.to_str().ok()?)) else {
            return false;
        };
        let (type_, subtype) = (range.type_().as_str(), range.subtype().as_str());
        let suffix = subtype
            .rsplit_once('+')
            .filter(|(base, _)| !base.is_empty())
            .map(|(_, suffix)| suffix);
        self.0.iter().any(|entry| match entry.kind() {
            MediaRangeKind::Any => true,
            MediaRangeKind::TypeWildcard => entry.type_().eq_ignore_ascii_case(type_),
            MediaRangeKind::Exact => {
                entry.type_().eq_ignore_ascii_case(type_)
                    && (entry.subtype().eq_ignore_ascii_case(subtype)
                        || suffix.is_some_and(|suffix| entry.subtype().eq_ignore_ascii_case(suffix)))
            }
        })
    }
}

/// The type and the subtype of the media type `text`, without its parameters.
fn media_range(text: &str) -> Option<MediaRange<'_>> {
    let essence = text.split_once(';').map_or(text, |(essence, _)| essence);
    MediaRange::parse(essence.trim_matches([' ', '\t'])).ok()
}
