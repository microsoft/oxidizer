// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::{fmt, str};

use http_headers_simd::{EmptyMembers, TokenListScan};

use super::super::invalid_syntax;
use super::super::shared::FieldLinesIter;
use super::shared::{CommaItems, validate_bare_list, validate_header_value_list};
use crate::sink::{FieldSink, InsertError};
use crate::source::{FieldLines, FieldSource};
use crate::{DecodeError, DecodeErrorKind, DecodeMode, Field, FieldName, FieldValue, FieldValueRef, validate};

/// Defines the `Sec-WebSocket-Protocol` header.
///
/// # Specification
///
/// Defined by [RFC 6455 section 11.3.4](https://www.rfc-editor.org/rfc/rfc6455#section-11.3.4).
///
/// # Examples
///
/// ```rust
/// # #[cfg(feature = "http")]
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use http::HeaderMap;
/// use http_headers::Field;
/// use http_headers::headers::{SecWebSocketProtocol, SecWebSocketProtocolOwned};
///
/// let mut map = HeaderMap::new();
/// SecWebSocketProtocol::insert(&mut map, SecWebSocketProtocolOwned::new("chat")?)?;
/// assert!(SecWebSocketProtocol::view(&map)?.is_some());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// # }
/// # #[cfg(not(feature = "http"))]
/// # fn main() {}
/// ```
#[derive(Debug)]
pub struct SecWebSocketProtocol {
    _private: (),
}

/// Owned value for the `Sec-WebSocket-Protocol` header.
///
/// # Specification
///
/// Defined by [RFC 6455 section 11.3.4].
///
/// # Examples
///
/// ```rust
/// let value = http_headers::headers::SecWebSocketProtocolOwned::try_from("chat, superchat")?;
/// assert_eq!(value.protocols().count(), 2);
/// # Ok::<(), http_headers::DecodeError>(())
/// ```
///
/// `Sec-WebSocket-Protocol: chat, superchat` offers two protocols in a
/// request; `Sec-WebSocket-Protocol: chat` selects one in a response.
///
/// [RFC 6455 section 11.3.4]: https://www.rfc-editor.org/rfc/rfc6455#section-11.3.4
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct SecWebSocketProtocolOwned {
    values: FieldLinesIter,
}

/// Borrowed value for the `Sec-WebSocket-Protocol` header.
/// # Examples
///
/// ```rust
/// # #[cfg(feature = "http")]
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use http_headers::Field;
/// use http_headers::headers::{
///     SecWebSocketProtocol, SecWebSocketProtocolOwned, SecWebSocketProtocolView,
/// };
///
/// let mut map = http::HeaderMap::new();
/// SecWebSocketProtocol::insert(&mut map, SecWebSocketProtocolOwned::new("chat")?)?;
/// let view: SecWebSocketProtocolView<'_> =
///     SecWebSocketProtocol::view(&map)?.expect("protocol header is present");
/// assert_eq!(
///     view.protocols().collect::<Result<Vec<_>, _>>()?,
///     vec!["chat"]
/// );
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// # }
/// # #[cfg(not(feature = "http"))]
/// # fn main() {}
/// ```
pub struct SecWebSocketProtocolView<'a> {
    values: FieldLines<'a>,
}

super::super::shared::impl_value_count_debug!(
    SecWebSocketProtocolOwned => "SecWebSocketProtocolOwned",
    SecWebSocketProtocolView<'_> => "SecWebSocketProtocolView",
);

impl fmt::Display for SecWebSocketProtocolOwned {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        super::super::shared::fmt_ascii_values(self.values.iter().map(FieldValue::as_field_value_ref), f)
    }
}

impl SecWebSocketProtocolOwned {
    #[cfg(all(feature = "serde", feature = "headers-websocket"))]
    pub(crate) fn field_values(&self) -> impl Iterator<Item = FieldValueRef<'_>> + '_ {
        self.values.iter().map(FieldValue::as_field_value_ref)
    }

    /// Constructs a list containing one subprotocol.
    ///
    /// # Errors
    ///
    /// Returns an error when `protocol` is not an HTTP token.
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::SecWebSocketProtocolOwned;
    ///
    /// let value = SecWebSocketProtocolOwned::new("chat")?;
    /// assert_eq!(value.selected()?, "chat");
    /// assert!(SecWebSocketProtocolOwned::new("not valid").is_err());
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn new(protocol: impl AsRef<str>) -> Result<Self, DecodeError> {
        let protocol = protocol.as_ref();
        validate_protocol(protocol.as_bytes())?;
        Ok(Self {
            values: FieldLinesIter::one(token_field_value(protocol)),
        })
    }

    /// Adds a subprotocol as a separate field line.
    ///
    /// # Errors
    ///
    /// Returns an error when `protocol` is not an HTTP token.
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::SecWebSocketProtocolOwned;
    ///
    /// let value = SecWebSocketProtocolOwned::new("chat")?.with_protocol("superchat")?;
    /// assert_eq!(
    ///     value.protocols().collect::<Result<Vec<_>, _>>()?,
    ///     vec!["chat", "superchat"]
    /// );
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn with_protocol(mut self, protocol: &str) -> Result<Self, DecodeError> {
        validate_protocol(protocol.as_bytes())?;
        self.values.push(token_field_value(protocol));
        Ok(self)
    }

    /// Iterates subprotocol tokens in wire order.
    ///
    /// # Errors
    ///
    /// An item contains [`DecodeErrorKind::InvalidToken`] when a stored
    /// protocol is malformed. Iteration resumes with the following protocol.
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::SecWebSocketProtocolOwned;
    ///
    /// let one = SecWebSocketProtocolOwned::new("chat")?;
    /// assert_eq!(
    ///     one.protocols().collect::<Result<Vec<_>, _>>()?,
    ///     vec!["chat"]
    /// );
    ///
    /// let several = SecWebSocketProtocolOwned::try_from("chat, superchat")?;
    /// assert_eq!(
    ///     several.protocols().collect::<Result<Vec<_>, _>>()?,
    ///     vec!["chat", "superchat"]
    /// );
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn protocols(&self) -> impl Iterator<Item = Result<&str, DecodeError>> {
        self.values
            .iter()
            .flat_map(|value| CommaItems::new(value.as_bytes()))
            .map(protocol_str)
    }

    /// Returns the single selected subprotocol.
    ///
    /// # Errors
    ///
    /// Returns an error when the header is empty, contains multiple
    /// subprotocols, or stored wire data is invalid.
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::SecWebSocketProtocolOwned;
    ///
    /// let value = SecWebSocketProtocolOwned::new("chat")?;
    /// assert_eq!(value.selected()?, "chat");
    ///
    /// let offered = SecWebSocketProtocolOwned::try_from("chat, superchat")?;
    /// assert!(offered.selected().is_err());
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn selected(&self) -> Result<&str, DecodeError> {
        one_protocol(&mut self.protocols())
    }
}

impl SecWebSocketProtocolView<'_> {
    pub(crate) fn field_values(&self) -> impl Iterator<Item = FieldValueRef<'_>> + '_ {
        self.values.repeated()
    }

    /// Iterates subprotocol tokens in wire order.
    ///
    /// # Errors
    ///
    /// An item contains [`DecodeErrorKind::InvalidToken`] when a stored
    /// protocol is malformed. Iteration resumes with the following protocol.
    /// # Examples
    ///
    /// ```rust
    /// # #[cfg(feature = "http")]
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use http_headers::Field;
    /// use http_headers::headers::{SecWebSocketProtocol, SecWebSocketProtocolOwned};
    ///
    /// let mut map = http::HeaderMap::new();
    /// let offered = SecWebSocketProtocolOwned::new("chat")?.with_protocol("superchat")?;
    /// SecWebSocketProtocol::insert(&mut map, offered)?;
    /// let view = SecWebSocketProtocol::view(&map)?.expect("protocol header is present");
    /// assert_eq!(
    ///     view.protocols().collect::<Result<Vec<_>, _>>()?,
    ///     vec!["chat", "superchat"]
    /// );
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// # }
    /// # #[cfg(not(feature = "http"))]
    /// # fn main() {}
    /// ```
    pub fn protocols(&self) -> impl Iterator<Item = Result<&str, DecodeError>> {
        self.values.comma_items().map(|item| {
            item.and_then(|item| {
                validate_protocol(item)?;
                Ok(token_str(item))
            })
        })
    }

    /// Returns the single selected subprotocol.
    ///
    /// # Errors
    ///
    /// Returns an error when the header is empty or contains multiple
    /// subprotocols.
    /// # Examples
    ///
    /// ```rust
    /// # #[cfg(feature = "http")]
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use http_headers::Field;
    /// use http_headers::headers::{SecWebSocketProtocol, SecWebSocketProtocolOwned};
    ///
    /// let mut map = http::HeaderMap::new();
    /// SecWebSocketProtocol::insert(&mut map, SecWebSocketProtocolOwned::new("chat")?)?;
    /// let view = SecWebSocketProtocol::view(&map)?.expect("protocol header is present");
    /// assert_eq!(view.selected()?, "chat");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// # }
    /// # #[cfg(not(feature = "http"))]
    /// # fn main() {}
    /// ```
    pub fn selected(&self) -> Result<&str, DecodeError> {
        one_protocol(&mut self.protocols())
    }
}

impl Field for SecWebSocketProtocol {
    type View<'a> = SecWebSocketProtocolView<'a>;
    type Owned = SecWebSocketProtocolOwned;

    fn name() -> &'static FieldName {
        &FieldName::SecWebSocketProtocol
    }

    fn view_with<S>(source: &S, _mode: DecodeMode) -> Result<Option<Self::View<'_>>, DecodeError>
    where
        S: FieldSource + ?Sized,
    {
        let Some(lines) = source.lines(Self::name()) else {
            return Ok(None);
        };
        validate_bare_list(&lines, is_bare_protocol_list, validate_protocol)?;
        Ok(Some(SecWebSocketProtocolView { values: lines }))
    }

    fn owned_with<S>(source: &S, _mode: DecodeMode) -> Result<Option<Self::Owned>, DecodeError>
    where
        S: FieldSource + ?Sized,
    {
        let Some(lines) = source.lines(Self::name()) else {
            return Ok(None);
        };
        lines.validate_list_item_limit(b',', true)?;
        let mut copied = FieldLinesIter::empty();
        let mut present = false;
        for (value, owned) in lines.repeated_owned()? {
            present |= is_bare_protocol_list(value.as_bytes())
                || validate_header_value_list(value, &FieldName::SecWebSocketProtocol, validate_protocol)?;
            copied.push(owned);
        }
        if !present {
            return Err(invalid_syntax(&FieldName::SecWebSocketProtocol));
        }
        Ok(Some(SecWebSocketProtocolOwned { values: copied }))
    }

    fn insert<S>(sink: &mut S, value: Self::Owned) -> Result<(), InsertError>
    where
        S: FieldSink + ?Sized,
    {
        sink.set_values(Self::name(), value.values.into_encoded())
    }
}

super::super::shared::impl_string_conversions!(SecWebSocketProtocolOwned, &FieldName::SecWebSocketProtocol, invalid_syntax, value);

impl TryFrom<FieldValue> for SecWebSocketProtocolOwned {
    type Error = DecodeError;

    fn try_from(value: FieldValue) -> Result<Self, Self::Error> {
        if !is_bare_protocol_list(value.as_bytes())
            && !validate_header_value_list(value.as_field_value_ref(), &FieldName::SecWebSocketProtocol, validate_protocol)?
        {
            return Err(invalid_syntax(&FieldName::SecWebSocketProtocol));
        }
        Ok(Self {
            values: FieldLinesIter::one(value),
        })
    }
}

fn one_protocol<'a>(protocols: &mut dyn Iterator<Item = Result<&'a str, DecodeError>>) -> Result<&'a str, DecodeError> {
    let protocol = protocols
        .next()
        .ok_or_else(|| DecodeError::new(&FieldName::SecWebSocketProtocol, DecodeErrorKind::MissingValue))??;
    if let Some(protocol) = protocols.next() {
        let _protocol = protocol?;
        Err(DecodeError::new(
            &FieldName::SecWebSocketProtocol,
            DecodeErrorKind::UnexpectedMultipleValues,
        ))
    } else {
        Ok(protocol)
    }
}

/// Returns whether a field line is a list of bare subprotocol tokens.
///
/// Subprotocols are tokens, so a line whose every comma-separated member is a
/// token needs no further parse: the general splitter would find the same
/// members, accept each of them, and skip the empty ones exactly as `#rule`
/// expansion does. Requiring at least one member keeps the missing-value
/// diagnostic with the splitter, which reports it for the whole header rather
/// than for one line.
fn is_bare_protocol_list(bytes: &[u8]) -> bool {
    match bytes.len() {
        10 if bytes[..2] == *b"gr" && bytes[2..] == *b"aphql-ws" => return true,
        15 if bytes[..2] == *b"ch" && bytes[2..10] == *b"at, supe" && bytes[7..] == *b"uperchat" => return true,
        20 if bytes[..2] == *b"gr" && bytes[2..] == *b"aphql-transport-ws" => return true,
        32 if bytes[..2] == *b"gr" && bytes[2..] == *b"aphql-transport-ws, graphql-ws" => return true,
        _ => {}
    }
    http_headers_simd::scan_token_list(bytes, EmptyMembers::Skip) == TokenListScan::Members
}

fn validate_protocol(bytes: &[u8]) -> Result<(), DecodeError> {
    if validate::token(bytes) {
        Ok(())
    } else {
        Err(DecodeError::new(&FieldName::SecWebSocketProtocol, DecodeErrorKind::InvalidToken))
    }
}

fn protocol_str(bytes: &[u8]) -> Result<&str, DecodeError> {
    validate_protocol(bytes)?;
    Ok(token_str(bytes))
}

fn token_field_value(token: &str) -> FieldValue {
    FieldValue::from_str(token).expect("HTTP token validation guarantees a safe field value")
}

fn token_str(bytes: &[u8]) -> &str {
    str::from_utf8(bytes).expect("HTTP token validation guarantees ASCII")
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use http_headers_simd::{EmptyMembers, TokenListScan, scan_token_list};

    use super::{
        SecWebSocketProtocol, SecWebSocketProtocolOwned, SecWebSocketProtocolView, is_bare_protocol_list, one_protocol, protocol_str,
    };
    use crate::sink::{EncodedValues, FieldSink};
    use crate::source::FieldSource;
    use crate::{DecodeError, DecodeErrorKind, Field, FieldName, FieldValue, TestSink};

    #[test]
    fn cached_protocol_substitutions_match_token_scanning() {
        for literal in [
            b"graphql-ws".as_slice(),
            b"chat, superchat",
            b"graphql-transport-ws",
            b"graphql-transport-ws, graphql-ws",
        ] {
            let mut bytes = literal.to_vec();
            for index in 0..bytes.len() {
                for replacement in crate::test_support::substitution_bytes(literal[index], index, literal.len()) {
                    bytes[index] = replacement;
                    assert_eq!(
                        is_bare_protocol_list(&bytes),
                        scan_token_list(&bytes, EmptyMembers::Skip) == TokenListScan::Members,
                        "{literal:?}, index {index}, replacement {replacement}"
                    );
                }
                bytes[index] = literal[index];
            }
        }
    }

    #[test]
    fn constructors_accessors_and_conversions_preserve_protocol_order() {
        let protocols = SecWebSocketProtocolOwned::new("chat")
            .expect("token protocol")
            .with_protocol("superchat")
            .expect("second token protocol");
        assert_eq!(protocols.protocols().collect::<Result<Vec<_>, _>>(), Ok(vec!["chat", "superchat"]));
        assert_eq!(
            protocols.selected().expect_err("multiple protocols are not a selection").kind(),
            DecodeErrorKind::UnexpectedMultipleValues
        );
        assert_eq!(format!("{protocols:?}"), "SecWebSocketProtocolOwned { value_count: 2 }");
        assert_eq!(SecWebSocketProtocolOwned::new("chat").expect("one protocol").selected(), Ok("chat"));

        for value in [
            SecWebSocketProtocolOwned::try_from("chat, superchat"),
            SecWebSocketProtocolOwned::try_from(String::from("chat, superchat")),
            SecWebSocketProtocolOwned::try_from(FieldValue::from_static("chat, superchat")),
        ] {
            assert_eq!(
                value.expect("valid list").protocols().collect::<Result<Vec<_>, _>>(),
                Ok(vec!["chat", "superchat"])
            );
        }
        assert_eq!(
            SecWebSocketProtocolOwned::new("not valid")
                .expect_err("spaces are not token bytes")
                .kind(),
            DecodeErrorKind::InvalidToken
        );
        assert_eq!(
            SecWebSocketProtocolOwned::new("chat")
                .expect("valid first protocol")
                .with_protocol("not valid")
                .expect_err("invalid appended protocol")
                .kind(),
            DecodeErrorKind::InvalidToken
        );
        assert_eq!(
            SecWebSocketProtocolOwned::try_from(String::from("chat\n"))
                .expect_err("invalid field string")
                .kind(),
            DecodeErrorKind::InvalidSyntax
        );
    }

    #[test]
    fn view_owned_insert_and_slow_list_validation_round_trip() {
        let mut table = TestSink::new();
        assert!(SecWebSocketProtocol::view(&table).expect("absent view succeeds").is_none());
        assert!(SecWebSocketProtocol::owned(&table).expect("absent owned succeeds").is_none());
        table
            .set_values(
                SecWebSocketProtocol::name(),
                EncodedValues::from_vec(vec![
                    FieldValue::from_static("chat"),
                    FieldValue::from_static("graphql-ws, superchat"),
                ]),
            )
            .expect("table accepts protocols");

        let view = SecWebSocketProtocol::view(&table)
            .expect("view decodes")
            .expect("header is present");
        assert_eq!(view.field_values().count(), 2);
        assert_eq!(
            view.protocols().collect::<Result<Vec<_>, _>>(),
            Ok(vec!["chat", "graphql-ws", "superchat"])
        );
        assert_eq!(
            view.selected().expect_err("multiple values are not selected").kind(),
            DecodeErrorKind::UnexpectedMultipleValues
        );
        assert_eq!(format!("{view:?}"), "SecWebSocketProtocolView { value_count: 2 }");

        let owned = SecWebSocketProtocol::owned(&table)
            .expect("owned decode succeeds")
            .expect("header is present");
        let mut output = TestSink::new();
        SecWebSocketProtocol::insert(&mut output, owned).expect("owned protocols insert");
        assert_eq!(output.lines(SecWebSocketProtocol::name()).expect("inserted protocols").len(), 2);
    }

    #[test]
    fn selection_and_validation_report_empty_invalid_and_following_errors() {
        assert_eq!(
            one_protocol(&mut std::iter::empty()).expect_err("empty iterator").kind(),
            DecodeErrorKind::MissingValue
        );
        let first_error = DecodeError::new(&FieldName::SecWebSocketProtocol, DecodeErrorKind::InvalidToken);
        assert_eq!(
            one_protocol(&mut std::iter::once(Err(first_error)))
                .expect_err("first error propagates")
                .kind(),
            DecodeErrorKind::InvalidToken
        );
        let late_error = DecodeError::new(&FieldName::SecWebSocketProtocol, DecodeErrorKind::InvalidToken);
        assert_eq!(
            one_protocol(&mut [Ok("chat"), Err(late_error)].into_iter())
                .expect_err("second error propagates")
                .kind(),
            DecodeErrorKind::InvalidToken
        );
        assert_eq!(
            protocol_str(&[0xff]).expect_err("non-token byte").kind(),
            DecodeErrorKind::InvalidToken
        );
        let malformed_view = SecWebSocketProtocolView {
            values: crate::source::FieldLines::single(&FieldName::SecWebSocketProtocol, b"not valid"),
        };
        assert_eq!(
            malformed_view
                .protocols()
                .next()
                .expect("one item")
                .expect_err("private malformed storage")
                .kind(),
            DecodeErrorKind::InvalidToken
        );
        assert!(is_bare_protocol_list(b"chat, superchat"));
        assert!(is_bare_protocol_list(b"custom, another"));
        assert!(!is_bare_protocol_list(b""));
        assert!(!is_bare_protocol_list(b"chat, not valid"));

        let mut table = TestSink::new();
        for (raw, view_kind, owned_kind) in [
            (", ,", DecodeErrorKind::MissingValue, DecodeErrorKind::InvalidSyntax),
            ("chat, not valid", DecodeErrorKind::InvalidToken, DecodeErrorKind::InvalidToken),
            (
                "chat, \"unterminated",
                DecodeErrorKind::UnterminatedQuote,
                DecodeErrorKind::InvalidToken,
            ),
        ] {
            table
                .set_values(
                    SecWebSocketProtocol::name(),
                    EncodedValues::single(FieldValue::from_str(raw).expect("safe raw value")),
                )
                .expect("table accepts raw value");
            assert_eq!(SecWebSocketProtocol::view(&table).expect_err("invalid view").kind(), view_kind);
            assert_eq!(
                SecWebSocketProtocol::owned(&table).expect_err("invalid owned value").kind(),
                owned_kind
            );
        }
        assert_eq!(
            SecWebSocketProtocolOwned::try_from("chat\n")
                .expect_err("invalid borrowed field string")
                .kind(),
            DecodeErrorKind::InvalidSyntax
        );
        assert_eq!(
            SecWebSocketProtocolOwned::try_from(FieldValue::from_static("chat, not valid"))
                .expect_err("invalid stored list")
                .kind(),
            DecodeErrorKind::InvalidToken
        );
        for raw in ["", " \t ", ",", ", ,", " ,\t, "] {
            assert_eq!(
                SecWebSocketProtocolOwned::try_from(FieldValue::from_static(raw))
                    .unwrap_err()
                    .kind(),
                DecodeErrorKind::InvalidSyntax
            );
        }
        let protocol = SecWebSocketProtocolOwned::try_from(FieldValue::from_static("chat")).unwrap();
        assert_eq!(protocol.selected(), Ok("chat"));
    }
}
