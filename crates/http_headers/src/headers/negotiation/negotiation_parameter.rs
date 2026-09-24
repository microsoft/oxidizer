// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::iter::{self, FusedIterator};
use std::slice;

use super::negotiation_token::NegotiationToken;
use super::shared::{QuotedItems, invalid_syntax, valid_parameter_value};
use crate::{DecodeError, FieldName, validate};

/// A validated token or quoted negotiation parameter value.
///
/// Values are bytes, not necessarily UTF-8. Equality and hashing preserve the
/// raw spelling; use [`decoded_bytes()`](Self::decoded_bytes) to compare
/// unescaped bytes.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct NegotiationParameterValue<'a> {
    raw: &'a [u8],
    contents: &'a [u8],
    quoted: bool,
}

impl<'a> NegotiationParameterValue<'a> {
    /// Validates the raw token or quoted-string spelling.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid quoting, escapes, controls or token bytes.
    pub fn parse(raw: &'a [u8]) -> Result<Self, DecodeError> {
        if !valid_parameter_value(raw) {
            return Err(invalid_syntax(&FieldName::Accept));
        }
        Ok(Self::from_validated(raw))
    }

    fn from_validated(raw: &'a [u8]) -> Self {
        let quoted = raw.first() == Some(&b'"');
        let contents = if quoted { &raw[1..raw.len() - 1] } else { raw };
        Self { raw, contents, quoted }
    }

    /// Returns the original spelling, including quotes and backslashes.
    #[must_use]
    pub const fn raw_bytes(self) -> &'a [u8] {
        self.raw
    }

    /// Whether the wire value was a quoted string rather than a token.
    #[must_use]
    pub const fn is_quoted(self) -> bool {
        self.quoted
    }

    /// Iterates decoded bytes without allocating or interpreting them as UTF-8.
    ///
    /// Quoted-pair backslashes are removed. Each new iterator traverses the
    /// value again; the parameter name and value boundaries are retained.
    #[must_use]
    pub fn decoded_bytes(self) -> impl FusedIterator<Item = u8> + 'a {
        let mut bytes = self.contents.iter().copied();
        iter::from_fn(move || Self::next_decoded(&mut bytes, self.quoted)).fuse()
    }

    fn next_decoded(bytes: &mut impl Iterator<Item = u8>, quoted: bool) -> Option<u8> {
        let byte = bytes.next()?;
        Some(if quoted && byte == b'\\' {
            bytes.next().expect("validated quoted pairs always contain an escaped byte")
        } else {
            byte
        })
    }

    /// Explicitly allocates the unescaped byte value.
    #[must_use]
    pub fn to_decoded_bytes(self) -> Vec<u8> {
        self.decoded_bytes().collect()
    }
}

/// A validated negotiation parameter name and optional byte-valued value.
///
/// An absent value is permitted for Accept extensions, but not media
/// parameters. [`AcceptEntry::new`](super::accept_entry::AcceptEntry::new)
/// validates that distinction and reserves the `q` name.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct NegotiationParameter<'a> {
    name: NegotiationToken<'a>,
    value: Option<NegotiationParameterValue<'a>>,
}

impl<'a> NegotiationParameter<'a> {
    /// Combines a validated name and optional validated value.
    #[must_use]
    pub const fn new(name: NegotiationToken<'a>, value: Option<NegotiationParameterValue<'a>>) -> Self {
        Self { name, value }
    }

    pub(super) fn from_validated(bytes: &'a [u8]) -> Self {
        let (name, value) = match bytes.iter().position(|byte| *byte == b'=') {
            Some(equals) => (
                validate::trim_ows(&bytes[..equals]),
                Some(NegotiationParameterValue::from_validated(validate::trim_ows(&bytes[equals + 1..]))),
            ),
            None => (bytes, None),
        };
        Self {
            name: NegotiationToken::from_validated(name),
            value,
        }
    }

    /// Returns the case-insensitive name token with original spelling.
    #[must_use]
    pub const fn name(self) -> NegotiationToken<'a> {
        self.name
    }

    /// Returns the value, distinguishing a bare extension from `name=""`.
    #[must_use]
    pub const fn value(self) -> Option<NegotiationParameterValue<'a>> {
        self.value
    }

    pub(super) fn encoded_len(self) -> usize {
        self.name.as_str().len() + self.value.map_or(0, |value| 1 + value.raw.len())
    }

    pub(super) fn append_to(self, bytes: &mut Vec<u8>) {
        bytes.push(b';');
        bytes.extend_from_slice(self.name.as_str().as_bytes());
        if let Some(value) = self.value {
            bytes.push(b'=');
            bytes.extend_from_slice(value.raw);
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) enum ParameterSource<'a> {
    Wire(&'a [u8]),
    Components(&'a [NegotiationParameter<'a>]),
}

impl<'a> ParameterSource<'a> {
    pub(super) fn iter(self) -> NegotiationParameters<'a> {
        let repr = match self {
            Self::Wire([]) => ParameterIterator::Empty,
            Self::Wire(bytes) => ParameterIterator::Wire(QuotedItems::semicolon(bytes, &FieldName::Accept)),
            Self::Components(parameters) => ParameterIterator::Components(parameters.iter()),
        };
        NegotiationParameters { repr }
    }
}

/// Allocation-free traversal of media parameters or Accept extensions.
///
/// Order and duplicates are preserved. Each item retains its component
/// boundaries, so its getters do not repeat parameter parsing.
#[derive(Debug)]
pub struct NegotiationParameters<'a> {
    repr: ParameterIterator<'a>,
}

#[derive(Debug)]
enum ParameterIterator<'a> {
    Empty,
    Wire(QuotedItems<'a>),
    Components(slice::Iter<'a, NegotiationParameter<'a>>),
}

impl<'a> Iterator for NegotiationParameters<'a> {
    type Item = NegotiationParameter<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        match &mut self.repr {
            ParameterIterator::Empty => None,
            ParameterIterator::Components(parameters) => parameters.next().copied(),
            ParameterIterator::Wire(parameters) => parameters
                .next()
                .map(|parameter| NegotiationParameter::from_validated(parameter.expect("header decoding validated every parameter"))),
        }
    }
}

impl FusedIterator for NegotiationParameters<'_> {}
