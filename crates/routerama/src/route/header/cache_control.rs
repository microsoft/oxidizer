// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Allocation-free `Cache-Control` parsing.
//!
//! The field grammar is `1#cache-directive` where
//! `cache-directive = token [ "=" ( token / quoted-string ) ]`
//! ([RFC 9111], Section 5.2). Directive names are matched in a
//! case-insensitive way, quoted-string arguments may contain commas and
//! `\`-escapes ([RFC 9110], Section 5.6.4), and a directive that cannot be
//! parsed is ignored without discarding the rest of the field.
//!
//! `proxy-revalidate` is ignored: [`CacheControl`] exposes no accessor for it.
//!
//! [RFC 9111]: https://www.rfc-editor.org/rfc/rfc9111.html#section-5.2
//! [RFC 9110]: https://www.rfc-editor.org/rfc/rfc9110.html#section-5.6.4

use core::time::Duration;

use headers::CacheControl;

use super::grammar::is_tchar;

/// Parses and combines `Cache-Control` field lines.
///
/// Directives that are unknown or cannot be parsed are ignored, so the result
/// reflects every directive the field does express.
pub(super) fn parse_all<'value, I>(values: I) -> CacheControl
where
    I: IntoIterator<Item = &'value [u8]>,
{
    let mut directives = Directives::new();
    for value in values {
        directives.apply(value);
    }
    directives.finish()
}

const NO_CACHE: u16 = 0b0000_0000_0001;
const NO_STORE: u16 = 0b0000_0000_0010;
const NO_TRANSFORM: u16 = 0b0000_0000_0100;
const ONLY_IF_CACHED: u16 = 0b0000_0000_1000;
const MUST_REVALIDATE: u16 = 0b0000_0001_0000;
const MUST_UNDERSTAND: u16 = 0b0000_0010_0000;
const PUBLIC: u16 = 0b0000_0100_0000;
const PRIVATE: u16 = 0b0000_1000_0000;
const IMMUTABLE: u16 = 0b0001_0000_0000;

/// The directives [`CacheControl`] records as a flag, paired with the bit each
/// one sets in [`Directives::flags`].
const FLAG_DIRECTIVES: [(&[u8], u16); 9] = [
    (b"no-cache", NO_CACHE),
    (b"no-store", NO_STORE),
    (b"no-transform", NO_TRANSFORM),
    (b"only-if-cached", ONLY_IF_CACHED),
    (b"must-revalidate", MUST_REVALIDATE),
    (b"must-understand", MUST_UNDERSTAND),
    (b"public", PUBLIC),
    (b"private", PRIVATE),
    (b"immutable", IMMUTABLE),
];

#[derive(Default)]
struct Directives {
    flags: u16,
    max_age: Option<u64>,
    max_stale: Option<u64>,
    min_fresh: Option<u64>,
    s_max_age: Option<u64>,
}

impl Directives {
    fn new() -> Self {
        Self::default()
    }

    fn apply(&mut self, value: &[u8]) {
        let mut cursor = Cursor::new(value);
        loop {
            cursor.skip_ows();
            while cursor.consume(b',') {
                cursor.skip_ows();
            }
            if cursor.is_empty() {
                return;
            }
            if self.directive(&mut cursor).is_none() {
                cursor.skip_directive();
            }
        }
    }

    /// Parses one directive, or returns [`None`] to leave it out of the result.
    ///
    /// The cursor always advances, either past the directive or up to the next
    /// delimiter, so the caller's loop terminates.
    fn directive(&mut self, cursor: &mut Cursor<'_>) -> Option<()> {
        let name = cursor.token()?;
        let argument = if cursor.consume(b'=') { Some(cursor.argument()?) } else { None };
        cursor.skip_ows();
        if !cursor.is_empty() && !cursor.at(b',') {
            return None;
        }
        self.record(name, argument);
        Some(())
    }

    fn record(&mut self, name: &[u8], argument: Option<&[u8]>) {
        // `no-cache` and `private` carry an optional field-name list. The
        // qualified form is recorded as the unqualified directive, which is the
        // conservative reading for a cache.
        let qualifiable = name.eq_ignore_ascii_case(b"no-cache") || name.eq_ignore_ascii_case(b"private");
        if argument.is_none() || qualifiable {
            for (directive, flag) in FLAG_DIRECTIVES {
                if name.eq_ignore_ascii_case(directive) {
                    self.flags |= flag;
                    return;
                }
            }
            return;
        }

        let Some(seconds) = argument.and_then(delta_seconds) else {
            return;
        };
        if name.eq_ignore_ascii_case(b"max-age") {
            self.max_age = Some(seconds);
        } else if name.eq_ignore_ascii_case(b"max-stale") {
            self.max_stale = Some(seconds);
        } else if name.eq_ignore_ascii_case(b"min-fresh") {
            self.min_fresh = Some(seconds);
        } else if name.eq_ignore_ascii_case(b"s-maxage") {
            self.s_max_age = Some(seconds);
        }
    }

    fn has(&self, flag: u16) -> bool {
        (self.flags & flag) != 0
    }

    fn finish(&self) -> CacheControl {
        let mut control = CacheControl::new();
        if self.has(NO_CACHE) {
            control = control.with_no_cache();
        }
        if self.has(NO_STORE) {
            control = control.with_no_store();
        }
        if self.has(NO_TRANSFORM) {
            control = control.with_no_transform();
        }
        if self.has(ONLY_IF_CACHED) {
            control = control.with_only_if_cached();
        }
        if self.has(MUST_REVALIDATE) {
            control = control.with_must_revalidate();
        }
        if self.has(MUST_UNDERSTAND) {
            control = control.with_must_understand();
        }
        if self.has(PUBLIC) {
            control = control.with_public();
        }
        if self.has(PRIVATE) {
            control = control.with_private();
        }
        if self.has(IMMUTABLE) {
            control = control.with_immutable();
        }
        if let Some(seconds) = self.max_age {
            control = control.with_max_age(Duration::from_secs(seconds));
        }
        if let Some(seconds) = self.max_stale {
            control = control.with_max_stale(Duration::from_secs(seconds));
        }
        if let Some(seconds) = self.min_fresh {
            control = control.with_min_fresh(Duration::from_secs(seconds));
        }
        if let Some(seconds) = self.s_max_age {
            control = control.with_s_max_age(Duration::from_secs(seconds));
        }
        control
    }
}

/// Parses RFC `delta-seconds`, saturating at [`u64::MAX`] as RFC 9111,
/// Section 1.2.2 permits for values a recipient cannot represent.
fn delta_seconds(value: &[u8]) -> Option<u64> {
    if value.is_empty() || !value.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let mut seconds: u64 = 0;
    for digit in value {
        seconds = seconds.saturating_mul(10).saturating_add(u64::from(digit - b'0'));
    }
    Some(seconds)
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

    fn at(&self, expected: u8) -> bool {
        self.value.get(self.offset) == Some(&expected)
    }

    fn consume(&mut self, expected: u8) -> bool {
        if self.at(expected) {
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

    /// Reads a directive argument in either the token or quoted-string form.
    fn argument(&mut self) -> Option<&'value [u8]> {
        if self.at(b'"') { self.quoted_string() } else { self.token() }
    }

    /// Reads a quoted string, returning its content with `\`-escapes intact.
    ///
    /// A quoted string with no closing quote consumes the rest of the field.
    fn quoted_string(&mut self) -> Option<&'value [u8]> {
        if !self.consume(b'"') {
            return None;
        }
        let start = self.offset;
        while let Some(byte) = self.value.get(self.offset) {
            match *byte {
                b'"' => {
                    let content = &self.value[start..self.offset];
                    self.offset += 1;
                    return Some(content);
                }
                // A quoted-pair escapes the next octet, including `"` and `\`.
                b'\\' => self.offset = self.value.len().min(self.offset + 2),
                _ => self.offset += 1,
            }
        }
        None
    }

    /// Advances to the delimiter that ends the current directive.
    ///
    /// Commas inside a quoted string do not delimit a directive, so the scan
    /// skips over quoted strings rather than splitting on every comma.
    fn skip_directive(&mut self) {
        while let Some(byte) = self.value.get(self.offset) {
            match *byte {
                b',' => return,
                b'"' => _ = self.quoted_string(),
                _ => self.offset += 1,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(value: &[u8]) -> CacheControl {
        parse_all([value])
    }

    #[test]
    fn directive_names_are_matched_case_insensitively() {
        let control = parse(b"No-Store, NO-CACHE, Max-Age=30, PuBlIc");
        assert!(control.no_store());
        assert!(control.no_cache());
        assert!(control.public());
        assert_eq!(control.max_age(), Some(Duration::from_secs(30)));
    }

    #[test]
    fn every_representable_directive_is_recognized() {
        let control = parse(
            b"no-cache, no-store, no-transform, only-if-cached, must-revalidate, must-understand, public, private, \
              immutable, max-age=1, max-stale=2, min-fresh=3, s-maxage=4",
        );
        assert!(control.no_cache());
        assert!(control.no_store());
        assert!(control.no_transform());
        assert!(control.only_if_cached());
        assert!(control.must_revalidate());
        assert!(control.must_understand());
        assert!(control.public());
        assert!(control.private());
        assert!(control.immutable());
        assert_eq!(control.max_age(), Some(Duration::from_secs(1)));
        assert_eq!(control.max_stale(), Some(Duration::from_secs(2)));
        assert_eq!(control.min_fresh(), Some(Duration::from_secs(3)));
        assert_eq!(control.s_max_age(), Some(Duration::from_secs(4)));
    }

    #[test]
    fn a_directive_that_cannot_be_parsed_does_not_discard_the_field() {
        for value in [
            &b"no-store, max-age=abc"[..],
            b"max-age=abc, no-store",
            b"no-store, bogus directive",
            b"=oops, no-store",
            b"no-store, max-age=",
            b"no-store, unclosed=\"quoted",
            b"no-store, \"quoted-name\"",
        ] {
            assert!(parse(value).no_store(), "{:?}", core::str::from_utf8(value));
        }
    }

    #[test]
    fn quoted_string_arguments_are_read_as_one_directive() {
        let control = parse(b"private=\"set-cookie, authorization\", no-cache=\"set-cookie\", max-age=5");
        assert!(control.private());
        assert!(control.no_cache());
        assert_eq!(control.max_age(), Some(Duration::from_secs(5)));
    }

    #[test]
    fn a_directive_inside_a_quoted_string_is_not_recognized() {
        let control = parse(b"private=\"x, no-store\"");
        assert!(control.private());
        assert!(!control.no_store());
    }

    #[test]
    fn escaped_quotes_do_not_terminate_a_quoted_string() {
        let control = parse(b"private=\"a\\\", no-store\", max-age=5");
        assert!(control.private());
        assert!(!control.no_store());
        assert_eq!(control.max_age(), Some(Duration::from_secs(5)));
    }

    #[test]
    fn delta_seconds_accept_the_quoted_form_and_saturate() {
        assert_eq!(parse(b"max-age=\"200\"").max_age(), Some(Duration::from_secs(200)));
        assert_eq!(
            parse(b"max-age=99999999999999999999999").max_age(),
            Some(Duration::from_secs(u64::MAX))
        );
    }

    #[test]
    fn arguments_are_rejected_where_the_grammar_defines_none() {
        let control = parse(b"public=1, no-store=\"x\", immutable=yes");
        assert!(!control.public());
        assert!(!control.no_store());
        assert!(!control.immutable());
    }

    #[test]
    fn unknown_directives_and_empty_elements_are_ignored() {
        let control = parse(b" , community=\"UCI\", , stale-while-revalidate=30, no-store ,");
        assert!(control.no_store());
        assert_eq!(control.max_age(), None);
    }

    #[test]
    fn a_repeated_directive_keeps_the_last_value() {
        let control = parse_all([&b"max-age=10"[..], b"max-age=20"]);
        assert_eq!(control.max_age(), Some(Duration::from_secs(20)));
    }

    #[test]
    fn an_empty_field_yields_no_directives() {
        assert_eq!(parse(b""), CacheControl::new());
        assert_eq!(parse(b"   "), CacheControl::new());
    }

    #[test]
    fn pathological_fields_terminate_without_directives() {
        for value in [
            &b"\"\"\"\"\""[..],
            b",,,,",
            b"=",
            b"\\",
            b"\"\\",
            b"=\"a,b\"",
            b"\t \t",
            b";;",
            b"a=\"\\\"\"",
        ] {
            assert_eq!(parse(value), CacheControl::new(), "{:?}", core::str::from_utf8(value));
        }
    }
}
