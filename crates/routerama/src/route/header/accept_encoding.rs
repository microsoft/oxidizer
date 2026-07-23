// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Allocation-free `Accept-Encoding` negotiation.

use super::grammar::{MAX_QUALITY, is_tchar, parse_quality};

/// The number of well-known content codings tracked by [`AcceptEncoding`].
const CODINGS: usize = 6;

/// A well-known HTTP content coding recognized by [`AcceptEncoding`].
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
#[non_exhaustive]
pub enum Encoding {
    /// The `gzip` (and legacy `x-gzip`) coding.
    Gzip,
    /// The Brotli `br` coding.
    Brotli,
    /// The `deflate` coding.
    Deflate,
    /// The Zstandard `zstd` coding.
    Zstd,
    /// The `compress` (and legacy `x-compress`) coding.
    Compress,
    /// The `identity` (no transformation) coding.
    Identity,
}

impl Encoding {
    const fn index(self) -> usize {
        match self {
            Self::Gzip => 0,
            Self::Brotli => 1,
            Self::Deflate => 2,
            Self::Zstd => 3,
            Self::Compress => 4,
            Self::Identity => 5,
        }
    }

    fn from_token(token: &[u8]) -> Option<Self> {
        if token.eq_ignore_ascii_case(b"gzip") || token.eq_ignore_ascii_case(b"x-gzip") {
            Some(Self::Gzip)
        } else if token.eq_ignore_ascii_case(b"br") {
            Some(Self::Brotli)
        } else if token.eq_ignore_ascii_case(b"deflate") {
            Some(Self::Deflate)
        } else if token.eq_ignore_ascii_case(b"zstd") {
            Some(Self::Zstd)
        } else if token.eq_ignore_ascii_case(b"compress") || token.eq_ignore_ascii_case(b"x-compress") {
            Some(Self::Compress)
        } else if token.eq_ignore_ascii_case(b"identity") {
            Some(Self::Identity)
        } else {
            None
        }
    }
}

/// A resolved `Accept-Encoding` decision over the well-known [`Encoding`] set.
///
/// Each coding carries its effective quality in `0..=1000`, after applying
/// explicit entries, wildcard fallback, and the default acceptability of
/// `identity`. A wildcard sets the quality of every coding the field does not
/// list explicitly, `identity` included; `identity` keeps the maximum quality
/// only when neither an explicit entry nor a wildcard applies to it.
/// The compact [`Copy`] result can be cached without allocation.
///
/// # Examples
///
/// ```
/// use routerama::route::header::{AcceptEncoding, Encoding};
///
/// let decision = AcceptEncoding::parse(b"br;q=1.0, gzip;q=0.8, *;q=0").expect("valid field");
/// assert_eq!(
///     decision.preferred([Encoding::Gzip, Encoding::Brotli]),
///     Some(Encoding::Brotli),
/// );
/// assert!(!decision.accepts(Encoding::Deflate));
/// ```
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct AcceptEncoding {
    qualities: [u16; CODINGS],
}

impl AcceptEncoding {
    /// Parses one `Accept-Encoding` field value.
    ///
    /// Returns [`None`] for malformed syntax. An empty value is valid and
    /// accepts only `identity`.
    #[must_use]
    pub fn parse(value: &[u8]) -> Option<Self> {
        let mut builder = Builder::new();
        builder.apply(value)?;
        Some(builder.finish())
    }

    /// Parses and combines multiple `Accept-Encoding` field lines.
    ///
    /// The field lines are folded in the order they appear in the message, and
    /// the entries within each line in the order they are written. When the
    /// same coding — or the `*` wildcard — appears more than once, the last
    /// entry wins and fully replaces the earlier one, which lets a client
    /// narrow an offer it made earlier: `gzip;q=1.0, gzip;q=0` refuses `gzip`.
    /// [RFC 9110] leaves duplicate codings undefined; honoring the client's
    /// final word is the safe direction because it never revives a refusal.
    ///
    /// Returns [`None`] if any field line is malformed.
    ///
    /// [RFC 9110]: https://www.rfc-editor.org/rfc/rfc9110#field.accept-encoding
    #[must_use]
    pub fn parse_all<'value, I>(values: I) -> Option<Self>
    where
        I: IntoIterator<Item = &'value [u8]>,
    {
        let mut builder = Builder::new();
        for value in values {
            builder.apply(value)?;
        }
        Some(builder.finish())
    }

    /// Returns the effective quality (`0..=1000`) for `encoding`.
    #[must_use]
    #[inline]
    pub fn quality(self, encoding: Encoding) -> u16 {
        self.qualities[encoding.index()]
    }

    /// Returns whether `encoding` has a nonzero quality.
    #[must_use]
    #[inline]
    pub fn accepts(self, encoding: Encoding) -> bool {
        self.quality(encoding) != 0
    }

    /// Selects the highest-quality supported coding, breaking ties by order.
    #[must_use]
    pub fn preferred<I>(self, supported: I) -> Option<Encoding>
    where
        I: IntoIterator<Item = Encoding>,
    {
        let mut best = None;
        for encoding in supported {
            let quality = self.quality(encoding);
            if quality != 0 && best.is_none_or(|(_, best_quality)| quality > best_quality) {
                best = Some((encoding, quality));
            }
        }
        best.map(|(encoding, _)| encoding)
    }
}

struct Builder {
    explicit: [Option<u16>; CODINGS],
    star: Option<u16>,
}

impl Builder {
    const fn new() -> Self {
        Self {
            explicit: [None; CODINGS],
            star: None,
        }
    }

    fn apply(&mut self, value: &[u8]) -> Option<()> {
        let mut cursor = Cursor::new(value);
        loop {
            cursor.skip_ows();
            while cursor.consume(b',') {
                cursor.skip_ows();
            }
            if cursor.is_empty() {
                return Some(());
            }
            self.directive(&mut cursor)?;
            cursor.skip_ows();
            if cursor.is_empty() {
                return Some(());
            }
            if !cursor.consume(b',') {
                return None;
            }
        }
    }

    fn directive(&mut self, cursor: &mut Cursor<'_>) -> Option<()> {
        let coding = cursor.token()?;
        cursor.skip_ows();
        let quality = if cursor.consume(b';') {
            cursor.skip_ows();
            if !cursor.token()?.eq_ignore_ascii_case(b"q") {
                return None;
            }
            if !cursor.consume(b'=') {
                return None;
            }
            parse_quality(cursor.token()?)?
        } else {
            MAX_QUALITY
        };

        if coding == b"*" {
            self.star = Some(quality);
        } else if let Some(encoding) = Encoding::from_token(coding) {
            self.explicit[encoding.index()] = Some(quality);
        }
        Some(())
    }

    fn finish(&self) -> AcceptEncoding {
        let mut qualities = [0; CODINGS];
        let identity = Encoding::Identity.index();
        for (index, quality) in qualities.iter_mut().enumerate() {
            // `identity` is acceptable by default, but a wildcard covers it
            // like any other coding that is not explicitly listed.
            let absent = if index == identity { MAX_QUALITY } else { 0 };
            *quality = self.explicit[index].or(self.star).unwrap_or(absent);
        }
        AcceptEncoding { qualities }
    }
}

struct Cursor<'value> {
    value: &'value [u8],
    offset: usize,
}

impl<'value> Cursor<'value> {
    const fn new(value: &'value [u8]) -> Self {
        Self { value, offset: 0 }
    }

    fn is_empty(&self) -> bool {
        self.offset == self.value.len()
    }

    fn consume(&mut self, expected: u8) -> bool {
        if self.value.get(self.offset) == Some(&expected) {
            self.offset += 1;
            true
        } else {
            false
        }
    }

    fn skip_ows(&mut self) {
        while self.value.get(self.offset).is_some_and(|byte| matches!(byte, b' ' | b'\t')) {
            self.offset += 1;
        }
    }

    fn token(&mut self) -> Option<&'value [u8]> {
        let start = self.offset;
        while self.value.get(self.offset).is_some_and(|byte| is_tchar(*byte)) {
            self.offset += 1;
        }
        (self.offset != start).then_some(&self.value[start..self.offset])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_value_accepts_only_identity() {
        let decision = AcceptEncoding::parse(b"").expect("empty is valid");
        assert!(decision.accepts(Encoding::Identity));
        assert!(!decision.accepts(Encoding::Gzip));
    }

    #[test]
    fn explicit_weights_aliases_and_wildcard_resolve() {
        let decision = AcceptEncoding::parse(b"x-gzip;q=0.5, br, *;q=0").expect("valid");
        assert_eq!(decision.quality(Encoding::Gzip), 500);
        assert_eq!(decision.quality(Encoding::Brotli), MAX_QUALITY);
        assert!(!decision.accepts(Encoding::Deflate));
        assert!(!decision.accepts(Encoding::Identity));
    }

    #[test]
    fn explicit_identity_overrides_the_wildcard() {
        let decision = AcceptEncoding::parse(b"identity;q=1, *;q=0").expect("valid");
        assert!(decision.accepts(Encoding::Identity));
        assert!(!decision.accepts(Encoding::Gzip));
    }

    #[test]
    fn positive_wildcard_lowers_identity_quality() {
        let decision = AcceptEncoding::parse(b"*;q=0.5").expect("valid");
        assert_eq!(decision.quality(Encoding::Gzip), 500);
        assert_eq!(decision.quality(Encoding::Identity), 500);
    }

    #[test]
    fn a_wildcard_weaker_than_an_explicit_coding_does_not_win_through_identity() {
        let decision = AcceptEncoding::parse(b"gzip;q=0.9, *;q=0.1").expect("valid");
        assert_eq!(decision.quality(Encoding::Identity), 100);
        assert_eq!(decision.preferred([Encoding::Gzip, Encoding::Identity]), Some(Encoding::Gzip));
    }

    #[test]
    fn a_field_without_a_wildcard_leaves_identity_at_max_quality() {
        let decision = AcceptEncoding::parse(b"gzip;q=0.5").expect("valid");
        assert_eq!(decision.quality(Encoding::Identity), MAX_QUALITY);
    }

    #[test]
    fn an_explicit_identity_overrides_a_positive_wildcard() {
        let decision = AcceptEncoding::parse(b"identity;q=0.9, *;q=0.1").expect("valid");
        assert_eq!(decision.quality(Encoding::Identity), 900);

        let decision = AcceptEncoding::parse(b"identity;q=0, *;q=0.9").expect("valid");
        assert!(!decision.accepts(Encoding::Identity));
    }

    #[test]
    fn preferred_uses_client_quality_then_server_order() {
        let decision = AcceptEncoding::parse(b"gzip;q=0.8, br;q=0.8, deflate;q=0.5").expect("valid");
        assert_eq!(decision.preferred([Encoding::Brotli, Encoding::Gzip]), Some(Encoding::Brotli));
        assert_eq!(decision.preferred([Encoding::Deflate]), Some(Encoding::Deflate));
        assert_eq!(decision.preferred([Encoding::Zstd]), None);
    }

    #[test]
    fn repeated_entries_and_field_lines_keep_the_last_quality() {
        let decision = AcceptEncoding::parse_all([&b"gzip;q=0.2, br;q=0.5"[..], b"gzip;q=0.8, br;q=0"]).expect("valid");
        assert_eq!(decision.quality(Encoding::Gzip), 800);
        assert_eq!(decision.quality(Encoding::Brotli), 0);
    }

    #[test]
    fn a_later_entry_for_the_same_coding_replaces_the_earlier_one() {
        let decision = AcceptEncoding::parse(b"gzip;q=1.0, gzip;q=0").expect("valid");
        assert_eq!(decision.quality(Encoding::Gzip), 0);

        let decision = AcceptEncoding::parse(b"gzip;q=0, gzip;q=1.0").expect("valid");
        assert_eq!(decision.quality(Encoding::Gzip), MAX_QUALITY);
    }

    #[test]
    fn a_later_wildcard_replaces_the_earlier_wildcard() {
        let decision = AcceptEncoding::parse(b"*;q=1.0, *;q=0").expect("valid");
        assert_eq!(decision.quality(Encoding::Deflate), 0);

        let decision = AcceptEncoding::parse(b"*;q=0, *;q=1.0").expect("valid");
        assert_eq!(decision.quality(Encoding::Deflate), MAX_QUALITY);
    }

    #[test]
    fn a_later_field_line_replaces_an_earlier_field_lines_coding() {
        let decision = AcceptEncoding::parse_all([&b"gzip;q=1.0, *;q=1.0"[..], b"gzip;q=0, *;q=0"]).expect("valid");
        assert_eq!(decision.quality(Encoding::Gzip), 0);
        assert_eq!(decision.quality(Encoding::Deflate), 0);

        let decision = AcceptEncoding::parse_all([&b"gzip;q=0, *;q=0"[..], b"gzip;q=1.0, *;q=1.0"]).expect("valid");
        assert_eq!(decision.quality(Encoding::Gzip), MAX_QUALITY);
        assert_eq!(decision.quality(Encoding::Deflate), MAX_QUALITY);
    }

    #[test]
    fn all_token_characters_are_accepted_for_extension_codings() {
        let decision = AcceptEncoding::parse(b"!#$%&'*+-.^_`|~;q=0.2, gzip").expect("valid");
        assert!(decision.accepts(Encoding::Gzip));
    }

    #[test]
    fn empty_fractional_qvalues_are_accepted() {
        let decision = AcceptEncoding::parse(b"gzip;q=0., br;q=1.").expect("valid");
        assert_eq!(decision.quality(Encoding::Gzip), 0);
        assert_eq!(decision.quality(Encoding::Brotli), MAX_QUALITY);
    }

    #[test]
    fn malformed_fields_are_rejected() {
        for value in [
            &b"gzip;"[..],
            b"gzip;x=1",
            b"gzip;q=2",
            b"gzip;q=0.0000",
            b"gzip;q=0.5x",
            b"gzip;q =0.5",
            b"gzip;q= 0.5",
            b"gzip br",
        ] {
            assert!(AcceptEncoding::parse(value).is_none(), "{:?}", core::str::from_utf8(value));
        }
    }
}
