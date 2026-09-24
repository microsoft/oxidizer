// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use super::media_range::MediaRange;
use super::negotiation_members::{append_weight, checked_len, weight_len};
use super::negotiation_parameter::{NegotiationParameter, NegotiationParameters, ParameterSource};
use super::quality::QualityView;
use super::shared::{QuotedItems, invalid_syntax};
use crate::{DecodeError, FieldName, validate};

/// One validated Accept preference with retained semantic components.
///
/// Scalar getters do not parse again. Parameter iteration traverses only its
/// retained region; each fresh iteration or lookup can traverse it again.
/// Media parameters precede `q`, and extensions follow it. Missing `q` means
/// effective quality one, but remains distinguishable from explicit `q=1`.
#[derive(Clone, Copy, Debug)]
pub struct AcceptEntry<'a> {
    range: MediaRange<'a>,
    quality: Option<QualityView<'a>>,
    parameters: ParameterSource<'a>,
    extensions: ParameterSource<'a>,
}

impl<'a> AcceptEntry<'a> {
    /// Constructs a preference from validated components.
    ///
    /// # Errors
    ///
    /// Media parameters require values. Neither parameter list may contain a
    /// `q` name, and extensions require an explicit quality to separate them
    /// from media parameters. The serialized member must fit
    /// [`MAX_CUSTOM_FIELD_BYTES`](crate::source::MAX_CUSTOM_FIELD_BYTES).
    pub fn new(
        range: MediaRange<'a>,
        parameters: &'a [NegotiationParameter<'a>],
        quality: Option<QualityView<'a>>,
        extensions: &'a [NegotiationParameter<'a>],
    ) -> Result<Self, DecodeError> {
        if parameters
            .iter()
            .any(|parameter| parameter.name().eq_ignore_ascii_case("q") || parameter.value().is_none())
            || extensions.iter().any(|parameter| parameter.name().eq_ignore_ascii_case("q"))
            || (!extensions.is_empty() && quality.is_none())
        {
            return Err(invalid_syntax(&FieldName::Accept));
        }
        let entry = Self {
            range,
            quality,
            parameters: ParameterSource::Components(parameters),
            extensions: ParameterSource::Components(extensions),
        };
        entry.encoded_len()?;
        Ok(entry)
    }

    pub(super) fn from_validated(bytes: &'a [u8]) -> Self {
        let head_end = bytes.iter().position(|byte| *byte == b';').unwrap_or(bytes.len());
        let range = MediaRange::from_validated(validate::trim_ows(&bytes[..head_end]));
        if head_end == bytes.len() {
            return Self {
                range,
                quality: None,
                parameters: ParameterSource::Wire(&[]),
                extensions: ParameterSource::Wire(&[]),
            };
        }
        let parameter_start = head_end + 1;
        if let [b'q' | b'Q', b'=', tail @ ..] = &bytes[parameter_start..] {
            // Header validation excludes quoted qualities, so the first
            // semicolon closes the quality rather than a quoted value.
            let (quality, extensions) = match tail.iter().position(|byte| *byte == b';') {
                Some(end) => (&tail[..end], validate::trim_ows(&tail[end + 1..])),
                None => (tail, &[][..]),
            };
            return Self {
                range,
                quality: Some(QualityView::from_validated(validate::trim_ows(quality))),
                parameters: ParameterSource::Wire(&[]),
                extensions: ParameterSource::Wire(extensions),
            };
        }
        let mut quality = None;
        let mut parameter_end = bytes.len();
        let mut extensions = &bytes[bytes.len()..];
        for segment in QuotedItems::semicolon(&bytes[parameter_start..], &FieldName::Accept) {
            let segment = segment.expect("header decoding validated every parameter's quoting");
            let equals = segment
                .iter()
                .position(|byte| *byte == b'=')
                .expect("validated media parameters before quality require values");
            if validate::trim_ows(&segment[..equals]).eq_ignore_ascii_case(b"q") {
                let start = segment.as_ptr().addr() - bytes.as_ptr().addr();
                parameter_end = bytes[..start]
                    .iter()
                    .rposition(|byte| *byte == b';')
                    .expect("a quality parameter follows a semicolon");
                let after_quality = start + segment.len();
                if let Some(delimiter) = bytes[after_quality..].iter().position(|byte| *byte == b';') {
                    extensions = &bytes[after_quality + delimiter + 1..];
                }
                quality = Some(QualityView::from_validated(validate::trim_ows(&segment[equals + 1..])));
                break;
            }
        }
        let parameters = if parameter_end < parameter_start {
            &bytes[0..0]
        } else {
            validate::trim_ows(&bytes[parameter_start..parameter_end])
        };
        Self {
            range,
            quality,
            parameters: ParameterSource::Wire(parameters),
            extensions: ParameterSource::Wire(validate::trim_ows(extensions)),
        }
    }

    /// Returns the validated media range.
    #[must_use]
    pub const fn range(self) -> MediaRange<'a> {
        self.range
    }

    /// Returns the exact effective quality, defaulting to one.
    #[must_use]
    pub const fn quality(self) -> QualityView<'a> {
        match self.quality {
            Some(quality) => quality,
            None => QualityView::ONE,
        }
    }

    /// Returns only an explicitly supplied quality.
    #[must_use]
    pub const fn explicit_quality(self) -> Option<QualityView<'a>> {
        self.quality
    }

    /// Iterates media parameters before `q`, preserving order and duplicates.
    #[must_use]
    pub fn parameters(self) -> NegotiationParameters<'a> {
        self.parameters.iter()
    }

    /// Iterates extensions after `q`, including extensions without a value.
    #[must_use]
    pub fn extensions(self) -> NegotiationParameters<'a> {
        self.extensions.iter()
    }

    pub(super) fn encoded_len(&self) -> Result<usize, DecodeError> {
        let components = [
            self.range.type_().as_str().len(),
            1,
            self.range.subtype().as_str().len(),
            weight_len(self.quality),
        ];
        checked_len(
            components.into_iter().chain(
                self.parameters()
                    .chain(self.extensions())
                    .map(|parameter| 1 + parameter.encoded_len()),
            ),
            &FieldName::Accept,
        )
    }

    pub(super) fn append_to(&self, bytes: &mut Vec<u8>) {
        bytes.extend_from_slice(self.range.type_().as_str().as_bytes());
        bytes.push(b'/');
        bytes.extend_from_slice(self.range.subtype().as_str().as_bytes());
        for parameter in self.parameters() {
            parameter.append_to(bytes);
        }
        append_weight(bytes, self.quality);
        for extension in self.extensions() {
            extension.append_to(bytes);
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod quality_first_tests {
    use crate::headers::Accept;
    use crate::source::{FieldLines, FieldSource};
    use crate::{DecodeErrorKind, DecodeMode, Field, FieldName};

    struct Source<'a>(&'a [u8]);

    impl FieldSource for Source<'_> {
        fn lines(&self, name: &'static FieldName) -> Option<FieldLines<'_>> {
            (name == &FieldName::Accept).then(|| FieldLines::single(name, self.0))
        }
    }

    fn compare_to_general(bytes: &[u8], mode: DecodeMode, expected_quality: &str, explicit: bool) {
        let source = Source(bytes);
        let decoded = Accept::view_with(&source, mode).unwrap().unwrap();
        let actual = decoded.entries().next().unwrap();
        let head_end = bytes.iter().position(|byte| *byte == b';').unwrap_or(bytes.len());
        let mut general_wire = bytes[..head_end].to_vec();
        // A leading media parameter forces the existing general projection.
        general_wire.extend_from_slice(b";comparison-probe=present");
        general_wire.extend_from_slice(&bytes[head_end..]);
        let general_source = Source(&general_wire);
        let general_decoded = Accept::view_with(&general_source, mode).unwrap().unwrap();
        let general = general_decoded.entries().next().unwrap();
        assert_eq!(actual.range(), general.range());
        assert_eq!(actual.quality(), general.quality());
        assert_eq!(actual.quality().to_string(), expected_quality);
        assert_eq!(actual.explicit_quality(), general.explicit_quality());
        assert_eq!(actual.explicit_quality().is_some(), explicit);
        let mut parameters = general.parameters();
        let probe = parameters.next().unwrap();
        assert_eq!(probe.name().as_str(), "comparison-probe");
        assert_eq!(probe.value().unwrap().raw_bytes(), b"present");
        assert_eq!(actual.parameters().collect::<Vec<_>>(), parameters.collect::<Vec<_>>());
        assert_eq!(actual.extensions().collect::<Vec<_>>(), general.extensions().collect::<Vec<_>>());
    }

    #[test]
    fn quality_first_and_general_projection_agree() {
        let cases: &[(&[u8], DecodeMode, &str, bool)] = &[
            (b"*/*", DecodeMode::Strict, "1", false),
            (b"text/plain;level=\"q=0.5;x\"", DecodeMode::Strict, "1", false),
            (b"*/*;q=0", DecodeMode::Strict, "0", true),
            (b"text/plain;q=0.", DecodeMode::Strict, "0", true),
            (b"text/*;Q=1.", DecodeMode::Strict, "1", true),
            (
                b"text/html;q=0.500;Flag;empty=\"\";tag=\"a,b;c\\\"\\\\\xff\";Flag",
                DecodeMode::Strict,
                "0.5",
                true,
            ),
            (b"text/plain;charset=utf-8;q=0.8;flag", DecodeMode::Strict, "0.8", true),
            (b"application/json; q=0.125 ;preview", DecodeMode::Strict, "0.125", true),
            (b"application/json;q=.5000;flag", DecodeMode::Relaxed, "0.5", true),
            (
                b"text/plain;q= \t.0001000000 \t;note=\"\xff;\\\"x\"",
                DecodeMode::Relaxed,
                "0.0001",
                true,
            ),
            (
                b"application/json;Q=0.5000000000000000000001;Flag",
                DecodeMode::Relaxed,
                "0.5000000000000000000001",
                true,
            ),
            (b"Text/HTML; q = .12500 ;Flag", DecodeMode::Relaxed, "0.125", true),
            (b"text/*;\tQ\t=\t1.0000;X=\"q=0;d,e\"", DecodeMode::Relaxed, "1", true),
        ];
        for &(bytes, mode, expected_quality, explicit) in cases {
            compare_to_general(bytes, mode, expected_quality, explicit);
        }
    }

    #[test]
    fn quality_first_retains_extension_order_and_byte_values() {
        let source = Source(b"text/html;q=0.500;Flag;empty=\"\";tag=\"a,b;c\\\"\\\\\xff\";Flag");
        let decoded = Accept::view(&source).unwrap().unwrap();
        let entry = decoded.entries().next().unwrap();
        assert_eq!(entry.parameters().count(), 0);
        let extensions = entry.extensions().collect::<Vec<_>>();
        assert_eq!(
            extensions.iter().map(|parameter| parameter.name().as_str()).collect::<Vec<_>>(),
            ["Flag", "empty", "tag", "Flag"]
        );
        assert_eq!(extensions[0].value(), None);
        assert_eq!(extensions[1].value().unwrap().to_decoded_bytes(), b"");
        assert_eq!(extensions[2].value().unwrap().to_decoded_bytes(), b"a,b;c\"\\\xff");
        assert_eq!(extensions[3].value(), None);
    }

    #[test]
    fn header_validation_still_rejects_invalid_quality_first_members() {
        for (bytes, kind) in [
            (b"text/plain;q=.5".as_slice(), DecodeErrorKind::InvalidSyntax),
            (b"text/plain;q=\"0.5\"", DecodeErrorKind::InvalidSyntax),
            (b"text/plain;q=0.5;Q=0.4", DecodeErrorKind::InvalidSyntax),
            (b"text/plain;flag", DecodeErrorKind::InvalidSyntax),
            (b"*/plain;q=0.5", DecodeErrorKind::InvalidSyntax),
            (b"text/plain;q=0.5;flag=\"unterminated", DecodeErrorKind::UnterminatedQuote),
        ] {
            let source = Source(bytes);
            let error = Accept::view(&source).unwrap_err();
            assert_eq!(error.kind(), kind);
            assert_eq!(error.value_index(), None);
        }
    }
}
