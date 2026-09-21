// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::{fmt, str};

use super::super::shared::FieldLinesIter;
use crate::sink::{FieldSink, InsertError};
use crate::source::{FieldLines, FieldSource};
use crate::{DecodeError, DecodeErrorKind, Field, FieldName, FieldValue, FieldValueRef, validate};

/// A recognized Referrer Policy token.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
/// # Examples
///
/// ```rust
/// let value = http_headers::headers::ReferrerPolicyOwned::new(
///     http_headers::headers::ReferrerPolicyValue::NoReferrer,
/// );
/// assert_eq!(
///     value.preferred()?,
///     http_headers::headers::ReferrerPolicyValue::NoReferrer
/// );
/// # Ok::<(), http_headers::DecodeError>(())
/// ```
pub enum ReferrerPolicyValue {
    /// Omits the `Referer` header.
    NoReferrer,
    /// Sends full referrers except on a secure-to-insecure downgrade.
    NoReferrerWhenDowngrade,
    /// Sends only the origin.
    Origin,
    /// Sends a full same-origin referrer and only the origin cross-origin.
    OriginWhenCrossOrigin,
    /// Sends referrers only for same-origin requests.
    SameOrigin,
    /// Sends the origin except on a secure-to-insecure downgrade.
    StrictOrigin,
    /// Sends a full same-origin referrer, an origin cross-origin, and nothing
    /// on a secure-to-insecure downgrade.
    StrictOriginWhenCrossOrigin,
    /// Sends the full referrer for all requests.
    UnsafeUrl,
}

/// One Referrer Policy token, including unrecognized extensions.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
/// # Examples
///
/// ```rust
/// use http_headers::headers::{
///     ReferrerPolicyOwned, ReferrerPolicyTokenView, ReferrerPolicyValue,
/// };
///
/// let value = ReferrerPolicyOwned::try_from("future-policy, strict-origin")?;
/// let mut tokens = value.tokens();
/// let extension: ReferrerPolicyTokenView<'_> = tokens.next().expect("extension token")?;
/// assert_eq!(extension.as_str(), "future-policy");
/// assert_eq!(extension.policy(), None);
/// let recognized = tokens.next().expect("recognized token")?;
/// assert_eq!(recognized.policy(), Some(ReferrerPolicyValue::StrictOrigin));
/// # Ok::<(), http_headers::DecodeError>(())
/// ```
pub struct ReferrerPolicyTokenView<'a> {
    token: &'a str,
    policy: Option<ReferrerPolicyValue>,
}

/// Defines the `Referrer-Policy` header.
///
/// # Specification
///
/// Defined by [Referrer Policy section 8.1](https://www.w3.org/TR/referrer-policy/#referrer-policy-header).
///
/// # Examples
///
/// ```rust
/// # #[cfg(feature = "http")]
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use http::HeaderMap;
/// use http_headers::Field;
/// use http_headers::headers::{ReferrerPolicy, ReferrerPolicyOwned, ReferrerPolicyValue};
///
/// let mut map = HeaderMap::new();
/// ReferrerPolicy::insert(
///     &mut map,
///     ReferrerPolicyOwned::new(ReferrerPolicyValue::NoReferrer),
/// )?;
/// assert!(ReferrerPolicy::view(&map)?.is_some());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// # }
/// # #[cfg(not(feature = "http"))]
/// # fn main() {}
/// ```
#[derive(Debug)]
pub struct ReferrerPolicy {
    _private: (),
}

/// Owned value for the `Referrer-Policy` header.
///
/// # Specification
///
/// Defined by [Referrer Policy section 8.1].
///
/// # Examples
///
/// ```rust
/// let value = http_headers::headers::ReferrerPolicyOwned::new(
///     http_headers::headers::ReferrerPolicyValue::NoReferrer,
/// );
/// assert_eq!(
///     value.preferred()?,
///     http_headers::headers::ReferrerPolicyValue::NoReferrer
/// );
/// # Ok::<(), http_headers::DecodeError>(())
/// ```
///
/// `Referrer-Policy: no-referrer` suppresses the referrer.
/// `Referrer-Policy: no-referrer, strict-origin-when-cross-origin` demonstrates
/// fallback-list processing, where the last recognized token is effective.
///
/// [Referrer Policy section 8.1]: https://www.w3.org/TR/referrer-policy/#referrer-policy-header
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct ReferrerPolicyOwned {
    values: FieldLinesIter,
}

/// Borrowed value for the `Referrer-Policy` header.
/// # Examples
///
/// ```rust
/// # #[cfg(feature = "http")]
/// # fn main() -> Result<(), http_headers::DecodeError> {
/// use http::{HeaderMap, HeaderValue};
/// use http_headers::Field;
/// use http_headers::headers::{ReferrerPolicy, ReferrerPolicyValue, ReferrerPolicyView};
///
/// let mut map = HeaderMap::new();
/// map.insert(
///     "referrer-policy",
///     HeaderValue::from_static("future-policy, strict-origin"),
/// );
/// let view: ReferrerPolicyView<'_> = ReferrerPolicy::view(&map)?.expect("header present");
/// assert_eq!(view.preferred()?, ReferrerPolicyValue::StrictOrigin);
/// # Ok::<(), http_headers::DecodeError>(())
/// # }
/// # #[cfg(not(feature = "http"))]
/// # fn main() {}
/// ```
pub struct ReferrerPolicyView<'a> {
    values: FieldLines<'a>,
}

super::super::shared::impl_value_count_debug!(
    ReferrerPolicyOwned => "ReferrerPolicyOwned",
    ReferrerPolicyView<'_> => "ReferrerPolicyView",
);

impl fmt::Display for ReferrerPolicyOwned {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        super::super::shared::fmt_ascii_values(self.values.iter().map(FieldValue::as_field_value_ref), f)
    }
}

impl ReferrerPolicyValue {
    /// Returns the serialized policy token.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::ReferrerPolicyValue;
    ///
    /// let policy = ReferrerPolicyValue::StrictOriginWhenCrossOrigin;
    /// assert_eq!(policy.as_str(), "strict-origin-when-cross-origin");
    /// ```
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NoReferrer => "no-referrer",
            Self::NoReferrerWhenDowngrade => "no-referrer-when-downgrade",
            Self::Origin => "origin",
            Self::OriginWhenCrossOrigin => "origin-when-cross-origin",
            Self::SameOrigin => "same-origin",
            Self::StrictOrigin => "strict-origin",
            Self::StrictOriginWhenCrossOrigin => "strict-origin-when-cross-origin",
            Self::UnsafeUrl => "unsafe-url",
        }
    }
}

impl<'a> ReferrerPolicyTokenView<'a> {
    /// Returns the original policy token.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::ReferrerPolicyOwned;
    ///
    /// let value = ReferrerPolicyOwned::try_from("future-policy, strict-origin")?;
    /// let token = value.tokens().next().expect("token present")?;
    /// assert_eq!(token.as_str(), "future-policy");
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub const fn as_str(self) -> &'a str {
        self.token
    }

    /// Returns the recognized policy, or `None` for a future extension token.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::{ReferrerPolicyOwned, ReferrerPolicyValue};
    ///
    /// let value = ReferrerPolicyOwned::try_from("future-policy, strict-origin")?;
    /// let policies = value
    ///     .tokens()
    ///     .map(|token| token.map(|token| token.policy()))
    ///     .collect::<Result<Vec<_>, _>>()?;
    /// assert_eq!(policies, [None, Some(ReferrerPolicyValue::StrictOrigin)]);
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub const fn policy(self) -> Option<ReferrerPolicyValue> {
        self.policy
    }
}

impl ReferrerPolicyOwned {
    /// Constructs a list containing one policy.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// let value = http_headers::headers::ReferrerPolicyOwned::new(
    ///     http_headers::headers::ReferrerPolicyValue::NoReferrer,
    /// );
    /// assert_eq!(
    ///     value.preferred()?,
    ///     http_headers::headers::ReferrerPolicyValue::NoReferrer
    /// );
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn new(policy: ReferrerPolicyValue) -> Self {
        Self {
            values: FieldLinesIter::one(FieldValue::from_static(policy.as_str())),
        }
    }

    /// Adds a fallback policy as another field line.
    #[must_use]
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::{ReferrerPolicyOwned, ReferrerPolicyValue};
    ///
    /// let value = ReferrerPolicyOwned::new(ReferrerPolicyValue::NoReferrer)
    ///     .with_fallback(ReferrerPolicyValue::StrictOriginWhenCrossOrigin);
    /// assert_eq!(
    ///     value.preferred()?,
    ///     ReferrerPolicyValue::StrictOriginWhenCrossOrigin,
    /// );
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn with_fallback(mut self, policy: ReferrerPolicyValue) -> Self {
        self.values.push(FieldValue::from_static(policy.as_str()));
        self
    }

    /// Iterates all tokens, including unrecognized extensions, in wire order.
    ///
    /// # Errors
    ///
    /// An item contains [`DecodeErrorKind::InvalidToken`] when malformed
    /// stored data is encountered. Iteration resumes with the following token.
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::ReferrerPolicyOwned;
    ///
    /// let value = ReferrerPolicyOwned::try_from("no-referrer, strict-origin")?;
    /// let tokens = value
    ///     .tokens()
    ///     .map(|token| token.map(|token| token.as_str()))
    ///     .collect::<Result<Vec<_>, _>>()?;
    /// assert_eq!(tokens, ["no-referrer", "strict-origin"]);
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn tokens(&self) -> impl Iterator<Item = Result<ReferrerPolicyTokenView<'_>, DecodeError>> {
        self.values
            .iter()
            .flat_map(|value| CommaItems::new(value.as_bytes()))
            .map(parse_referrer_policy_token)
    }

    /// Iterates recognized policies in wire order.
    ///
    /// # Errors
    ///
    /// Items propagate the token errors documented by [`Self::tokens`].
    /// Iteration resumes with the following token.
    /// # Examples
    ///
    /// ```rust
    /// use http_headers::headers::{ReferrerPolicyOwned, ReferrerPolicyValue};
    ///
    /// let value = ReferrerPolicyOwned::try_from(
    ///     "future-policy, no-referrer, strict-origin-when-cross-origin",
    /// )?;
    /// let policies = value.policies().collect::<Result<Vec<_>, _>>()?;
    /// assert_eq!(
    ///     policies,
    ///     [
    ///         ReferrerPolicyValue::NoReferrer,
    ///         ReferrerPolicyValue::StrictOriginWhenCrossOrigin,
    ///     ],
    /// );
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn policies(&self) -> impl Iterator<Item = Result<ReferrerPolicyValue, DecodeError>> + '_ {
        self.tokens().filter_map(recognized_policy)
    }

    /// Returns the preferred policy from a fallback list.
    ///
    /// # Errors
    ///
    /// Returns an error if stored wire data is invalid or no recognized
    /// policy is present.
    /// # Examples
    ///
    /// ```rust
    /// let value = http_headers::headers::ReferrerPolicyOwned::new(
    ///     http_headers::headers::ReferrerPolicyValue::NoReferrer,
    /// );
    /// assert_eq!(
    ///     value.preferred()?,
    ///     http_headers::headers::ReferrerPolicyValue::NoReferrer
    /// );
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn preferred(&self) -> Result<ReferrerPolicyValue, DecodeError> {
        self.policies()
            .last()
            .transpose()?
            .ok_or_else(|| super::super::invalid_syntax(&FieldName::ReferrerPolicy))
    }

    #[cfg(all(feature = "serde", feature = "headers-security"))]
    pub(crate) fn field_values(&self) -> impl Iterator<Item = FieldValueRef<'_>> + '_ {
        self.values.iter().map(FieldValue::as_field_value_ref)
    }
}

impl ReferrerPolicyView<'_> {
    pub(crate) fn field_values(&self) -> impl Iterator<Item = FieldValueRef<'_>> + '_ {
        self.values.repeated()
    }

    /// Iterates all tokens, including unrecognized extensions, in wire order.
    ///
    /// # Errors
    ///
    /// An item contains [`DecodeErrorKind::InvalidToken`] when malformed
    /// stored data is encountered. Iteration resumes with the following token.
    /// # Examples
    ///
    /// ```rust
    /// # #[cfg(feature = "http")]
    /// # fn main() -> Result<(), http_headers::DecodeError> {
    /// use http::{HeaderMap, HeaderValue};
    /// use http_headers::Field;
    /// use http_headers::headers::ReferrerPolicy;
    ///
    /// let mut map = HeaderMap::new();
    /// map.insert(
    ///     "referrer-policy",
    ///     HeaderValue::from_static("future-policy, no-referrer"),
    /// );
    /// let view = ReferrerPolicy::view(&map)?.expect("header present");
    /// let tokens = view
    ///     .tokens()
    ///     .map(|token| token.map(|token| token.as_str()))
    ///     .collect::<Result<Vec<_>, _>>()?;
    /// assert_eq!(tokens, ["future-policy", "no-referrer"]);
    /// # Ok::<(), http_headers::DecodeError>(())
    /// # }
    /// # #[cfg(not(feature = "http"))]
    /// # fn main() {}
    /// ```
    pub fn tokens(&self) -> impl Iterator<Item = Result<ReferrerPolicyTokenView<'_>, DecodeError>> + '_ {
        self.values.comma_items().map(|item| item.and_then(parse_referrer_policy_token))
    }

    /// Iterates recognized policies in wire order.
    ///
    /// # Errors
    ///
    /// Items propagate the token errors documented by [`Self::tokens`].
    /// Iteration resumes with the following token.
    /// # Examples
    ///
    /// ```rust
    /// # #[cfg(feature = "http")]
    /// # fn main() -> Result<(), http_headers::DecodeError> {
    /// use http::{HeaderMap, HeaderValue};
    /// use http_headers::Field;
    /// use http_headers::headers::{ReferrerPolicy, ReferrerPolicyValue};
    ///
    /// let mut map = HeaderMap::new();
    /// map.append(
    ///     "referrer-policy",
    ///     HeaderValue::from_static("future-policy, origin"),
    /// );
    /// map.append("referrer-policy", HeaderValue::from_static("strict-origin"));
    /// let view = ReferrerPolicy::view(&map)?.expect("header present");
    /// let policies = view.policies().collect::<Result<Vec<_>, _>>()?;
    /// assert_eq!(
    ///     policies,
    ///     [
    ///         ReferrerPolicyValue::Origin,
    ///         ReferrerPolicyValue::StrictOrigin,
    ///     ],
    /// );
    /// # Ok::<(), http_headers::DecodeError>(())
    /// # }
    /// # #[cfg(not(feature = "http"))]
    /// # fn main() {}
    /// ```
    pub fn policies(&self) -> impl Iterator<Item = Result<ReferrerPolicyValue, DecodeError>> + '_ {
        self.tokens().filter_map(recognized_policy)
    }

    /// Returns the preferred policy from a fallback list.
    ///
    /// # Errors
    ///
    /// Returns an error if stored wire data is invalid or no recognized
    /// policy is present.
    /// # Examples
    ///
    /// ```rust
    /// let value = http_headers::headers::ReferrerPolicyOwned::new(
    ///     http_headers::headers::ReferrerPolicyValue::NoReferrer,
    /// );
    /// assert_eq!(
    ///     value.preferred()?,
    ///     http_headers::headers::ReferrerPolicyValue::NoReferrer
    /// );
    /// # Ok::<(), http_headers::DecodeError>(())
    /// ```
    pub fn preferred(&self) -> Result<ReferrerPolicyValue, DecodeError> {
        self.policies()
            .last()
            .transpose()?
            .ok_or_else(|| super::super::invalid_syntax(&FieldName::ReferrerPolicy))
    }
}

impl Field for ReferrerPolicy {
    type View<'a> = ReferrerPolicyView<'a>;
    type Owned = ReferrerPolicyOwned;

    fn name() -> &'static FieldName {
        &FieldName::ReferrerPolicy
    }

    fn view_with<S>(source: &S, _mode: crate::DecodeMode) -> Result<Option<Self::View<'_>>, DecodeError>
    where
        S: FieldSource + ?Sized,
    {
        let Some(lines) = source.lines(Self::name()) else {
            return Ok(None);
        };
        lines.validate_custom_source()?;
        let mut all_recognized = false;
        for value in lines.repeated() {
            if recognize_referrer_policy(super::super::trim_ows(value.as_bytes())).is_none() {
                all_recognized = false;
                break;
            }
            all_recognized = true;
        }
        if all_recognized {
            return Ok(Some(ReferrerPolicyView { values: lines }));
        }

        let mut count = 0_usize;
        for item in lines.comma_items() {
            parse_referrer_policy_token(item?)?;
            count = increment_item_count(count)?;
        }
        if count == 0 {
            return Err(DecodeError::new(&FieldName::ReferrerPolicy, DecodeErrorKind::MissingValue));
        }
        Ok(Some(ReferrerPolicyView { values: lines }))
    }

    fn owned_with<S>(source: &S, _mode: crate::DecodeMode) -> Result<Option<Self::Owned>, DecodeError>
    where
        S: FieldSource + ?Sized,
    {
        let Some(lines) = source.lines(Self::name()) else {
            return Ok(None);
        };
        lines.validate_custom_source()?;
        let mut all_recognized = false;
        for value in lines.repeated() {
            if recognize_referrer_policy(super::super::trim_ows(value.as_bytes())).is_none() {
                all_recognized = false;
                break;
            }
            all_recognized = true;
        }
        let count = if all_recognized {
            lines.len()
        } else {
            let mut count = 0_usize;
            for item in lines.comma_items() {
                parse_referrer_policy_token(item?)?;
                count = increment_item_count(count)?;
            }
            count
        };
        if count == 0 {
            return Err(DecodeError::new(&FieldName::ReferrerPolicy, DecodeErrorKind::MissingValue));
        }
        let mut copied = FieldLinesIter::empty();
        for (_value, owned) in lines.repeated_owned()? {
            copied.push(owned);
        }
        Ok(Some(ReferrerPolicyOwned { values: copied }))
    }

    fn insert<S>(sink: &mut S, value: Self::Owned) -> Result<(), InsertError>
    where
        S: FieldSink + ?Sized,
    {
        sink.set_values(Self::name(), value.values.into_encoded())
    }
}

super::super::shared::impl_string_conversions!(ReferrerPolicyOwned, &FieldName::ReferrerPolicy, super::super::invalid_syntax, value);

impl TryFrom<FieldValue> for ReferrerPolicyOwned {
    type Error = DecodeError;

    fn try_from(value: FieldValue) -> Result<Self, Self::Error> {
        if recognize_referrer_policy(super::super::trim_ows(value.as_bytes())).is_some() {
            return Ok(Self {
                values: FieldLinesIter::one(value),
            });
        }

        let mut count = 0_usize;
        for item in CommaItems::new(value.as_bytes()) {
            parse_referrer_policy_token(item)?;
            count = increment_item_count(count)?;
        }
        if count == 0 {
            return Err(super::super::invalid_syntax(&FieldName::ReferrerPolicy));
        }
        Ok(Self {
            values: FieldLinesIter::one(value),
        })
    }
}

fn parse_referrer_policy_token(bytes: &[u8]) -> Result<ReferrerPolicyTokenView<'_>, DecodeError> {
    if let Some(policy) = recognize_referrer_policy(bytes) {
        return Ok(ReferrerPolicyTokenView {
            token: policy.as_str(),
            policy: Some(policy),
        });
    }
    if !validate::token(bytes) {
        return Err(DecodeError::new(&FieldName::ReferrerPolicy, DecodeErrorKind::InvalidToken));
    }
    let token = str::from_utf8(bytes).expect("HTTP token validation guarantees ASCII");
    Ok(ReferrerPolicyTokenView { token, policy: None })
}

fn increment_item_count(count: usize) -> Result<usize, DecodeError> {
    count
        .checked_add(1)
        .ok_or_else(|| DecodeError::new(&FieldName::ReferrerPolicy, DecodeErrorKind::InvalidNumber))
}

fn recognize_referrer_policy(bytes: &[u8]) -> Option<ReferrerPolicyValue> {
    if bytes == b"strict-origin-when-cross-origin" {
        return Some(ReferrerPolicyValue::StrictOriginWhenCrossOrigin);
    }
    match bytes.len() {
        11 => match bytes[0] {
            b'n' if &bytes[1..3] == b"o-" && &bytes[3..] == b"referrer" => Some(ReferrerPolicyValue::NoReferrer),
            b's' if bytes == b"same-origin" => Some(ReferrerPolicyValue::SameOrigin),
            _ => None,
        },
        26 if bytes.starts_with(b"no-") && bytes == b"no-referrer-when-downgrade" => Some(ReferrerPolicyValue::NoReferrerWhenDowngrade),
        6 if bytes[0] == b'o' && bytes == b"origin" => Some(ReferrerPolicyValue::Origin),
        24 if bytes[0] == b'o' && bytes == b"origin-when-cross-origin" => Some(ReferrerPolicyValue::OriginWhenCrossOrigin),
        13 if bytes[0] == b's' && bytes == b"strict-origin" => Some(ReferrerPolicyValue::StrictOrigin),
        10 if bytes[0] == b'u' && bytes == b"unsafe-url" => Some(ReferrerPolicyValue::UnsafeUrl),
        _ => None,
    }
}

fn recognized_policy(token: Result<ReferrerPolicyTokenView<'_>, DecodeError>) -> Option<Result<ReferrerPolicyValue, DecodeError>> {
    match token {
        Ok(token) => token.policy().map(Ok),
        Err(error) => Some(Err(error)),
    }
}

struct CommaItems<'a> {
    bytes: &'a [u8],
    start: usize,
    position: usize,
    finished: bool,
}

impl<'a> CommaItems<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            start: 0,
            position: 0,
            finished: false,
        }
    }
}

impl<'a> Iterator for CommaItems<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<Self::Item> {
        while !self.finished {
            if let Some(relative) = self.bytes[self.position..].iter().position(|byte| *byte == b',') {
                let end = self.position + relative;
                let item = super::super::trim_ows(&self.bytes[self.start..end]);
                self.position = end + 1;
                self.start = self.position;
                if item.is_empty() {
                    continue;
                }
                return Some(item);
            }
            self.finished = true;
            let item = super::super::trim_ows(&self.bytes[self.start..]);
            if !item.is_empty() {
                return Some(item);
            }
        }
        None
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::{
        CommaItems, ReferrerPolicy, ReferrerPolicyOwned, ReferrerPolicyTokenView, ReferrerPolicyValue, increment_item_count,
        parse_referrer_policy_token, recognize_referrer_policy,
    };
    use crate::sink::{EncodedValues, FieldSink};
    use crate::source::FieldSource;
    use crate::{DecodeErrorKind, Field, FieldValue, TestSink};

    #[test]
    fn recognized_values_and_token_accessors_cover_every_policy() {
        assert_eq!(
            increment_item_count(usize::MAX - 1).expect("last count is representable"),
            usize::MAX
        );
        assert_eq!(
            increment_item_count(usize::MAX).expect_err("count overflow is rejected").kind(),
            DecodeErrorKind::InvalidNumber
        );

        let policies = [
            (ReferrerPolicyValue::NoReferrer, "no-referrer"),
            (ReferrerPolicyValue::NoReferrerWhenDowngrade, "no-referrer-when-downgrade"),
            (ReferrerPolicyValue::Origin, "origin"),
            (ReferrerPolicyValue::OriginWhenCrossOrigin, "origin-when-cross-origin"),
            (ReferrerPolicyValue::SameOrigin, "same-origin"),
            (ReferrerPolicyValue::StrictOrigin, "strict-origin"),
            (ReferrerPolicyValue::StrictOriginWhenCrossOrigin, "strict-origin-when-cross-origin"),
            (ReferrerPolicyValue::UnsafeUrl, "unsafe-url"),
        ];
        for (policy, wire) in policies {
            assert_eq!(policy.as_str(), wire);
            let token = parse_referrer_policy_token(wire.as_bytes()).expect("recognized policy parses");
            assert_eq!(token.as_str(), wire);
            assert_eq!(token.policy(), Some(policy));
            let mut neighbor = wire.as_bytes().to_vec();
            for index in 0..neighbor.len() {
                for byte in crate::test_support::substitution_bytes(wire.as_bytes()[index], index, wire.len()) {
                    neighbor[index] = byte;
                    assert_eq!(
                        recognize_referrer_policy(&neighbor),
                        (byte == wire.as_bytes()[index]).then_some(policy),
                        "{neighbor:?}"
                    );
                }
                neighbor[index] = wire.as_bytes()[index];
            }
        }

        let extension = parse_referrer_policy_token(b"future-policy").expect("extension token parses");
        assert_eq!(extension.as_str(), "future-policy");
        assert_eq!(extension.policy(), None);
        assert_eq!(
            parse_referrer_policy_token(b"not a token")
                .expect_err("spaces are not token bytes")
                .kind(),
            DecodeErrorKind::InvalidToken
        );
    }

    #[test]
    fn constructors_fallbacks_and_conversions_preserve_wire_order() {
        let policy = ReferrerPolicyOwned::new(ReferrerPolicyValue::NoReferrer)
            .with_fallback(ReferrerPolicyValue::StrictOrigin)
            .with_fallback(ReferrerPolicyValue::UnsafeUrl);
        assert_eq!(
            policy.policies().collect::<Result<Vec<_>, _>>(),
            Ok(vec![
                ReferrerPolicyValue::NoReferrer,
                ReferrerPolicyValue::StrictOrigin,
                ReferrerPolicyValue::UnsafeUrl,
            ])
        );
        assert_eq!(policy.preferred(), Ok(ReferrerPolicyValue::UnsafeUrl));
        assert_eq!(format!("{policy:?}"), "ReferrerPolicyOwned { value_count: 3 }");

        for converted in [
            ReferrerPolicyOwned::try_from("future, origin"),
            ReferrerPolicyOwned::try_from(String::from("future, origin")),
            ReferrerPolicyOwned::try_from(FieldValue::from_static("future, origin")),
        ] {
            let converted = converted.expect("valid fallback list");
            let tokens = converted
                .tokens()
                .map(|token| token.map(ReferrerPolicyTokenView::as_str))
                .collect::<Result<Vec<_>, _>>();
            assert_eq!(tokens, Ok(vec!["future", "origin"]));
            assert_eq!(converted.preferred(), Ok(ReferrerPolicyValue::Origin));
        }
        assert_eq!(
            ReferrerPolicyOwned::try_from(FieldValue::from_static("origin"))
                .expect("recognized field value")
                .preferred(),
            Ok(ReferrerPolicyValue::Origin)
        );

        assert_eq!(
            ReferrerPolicyOwned::try_from(" , \t, ").expect_err("empty list").kind(),
            DecodeErrorKind::InvalidSyntax
        );
        assert_eq!(
            ReferrerPolicyOwned::try_from(String::from("origin\n"))
                .expect_err("invalid field value")
                .kind(),
            DecodeErrorKind::InvalidSyntax
        );
        assert_eq!(
            ReferrerPolicyOwned::try_from("origin\n")
                .expect_err("invalid borrowed field value")
                .kind(),
            DecodeErrorKind::InvalidSyntax
        );
        assert_eq!(
            ReferrerPolicyOwned::try_from("origin, not valid")
                .expect_err("invalid token")
                .kind(),
            DecodeErrorKind::InvalidToken
        );
        assert_eq!(
            ReferrerPolicyOwned::try_from("future")
                .expect("extension-only list is syntactically valid")
                .preferred()
                .expect_err("no recognized policy")
                .kind(),
            DecodeErrorKind::InvalidSyntax
        );
    }

    #[test]
    fn borrowed_and_owned_decoders_share_comma_list_classification() {
        let mut table = TestSink::new();
        assert!(ReferrerPolicy::view(&table).expect("absent view succeeds").is_none());
        assert!(ReferrerPolicy::owned(&table).expect("absent owned decode succeeds").is_none());

        table
            .set_values(
                ReferrerPolicy::name(),
                EncodedValues::from_vec(vec![
                    FieldValue::from_static("no-referrer"),
                    FieldValue::from_static("future, origin"),
                ]),
            )
            .expect("table accepts policies");
        let view = ReferrerPolicy::view(&table).expect("view decodes").expect("header is present");
        assert_eq!(view.field_values().count(), 2);
        assert_eq!(
            view.tokens()
                .map(|token| token.map(ReferrerPolicyTokenView::as_str))
                .collect::<Result<Vec<_>, _>>(),
            Ok(vec!["no-referrer", "future", "origin"])
        );
        assert_eq!(view.preferred(), Ok(ReferrerPolicyValue::Origin));
        assert_eq!(format!("{view:?}"), "ReferrerPolicyView { value_count: 2 }");

        let owned = ReferrerPolicy::owned(&table)
            .expect("owned value decodes")
            .expect("header is present");
        let mut output = TestSink::new();
        ReferrerPolicy::insert(&mut output, owned).expect("owned value inserts");
        assert_eq!(output.lines(ReferrerPolicy::name()).expect("inserted values").len(), 2);

        for raw in [" , ", "origin, not valid", "\"origin"] {
            table
                .set_values(
                    ReferrerPolicy::name(),
                    EncodedValues::single(FieldValue::from_str(raw).expect("safe field value")),
                )
                .expect("table accepts raw value");
            let view_error = ReferrerPolicy::view(&table).expect_err("view rejects invalid list");
            let owned_error = ReferrerPolicy::owned(&table).expect_err("owned rejects invalid list");
            assert_eq!(view_error.kind(), owned_error.kind());
        }

        table
            .set_values(
                ReferrerPolicy::name(),
                EncodedValues::single(FieldValue::from_static("future-policy")),
            )
            .expect("table accepts extension policy");
        assert_eq!(
            ReferrerPolicy::view(&table)
                .expect("extension view decodes")
                .expect("header is present")
                .preferred()
                .expect_err("no recognized policy")
                .kind(),
            DecodeErrorKind::InvalidSyntax
        );

        table
            .set_values(
                ReferrerPolicy::name(),
                EncodedValues::from_vec(vec![
                    FieldValue::from_static(" no-referrer "),
                    FieldValue::from_static("strict-origin"),
                ]),
            )
            .expect("table accepts recognized policies");
        assert_eq!(
            ReferrerPolicy::view(&table)
                .expect("recognized fast-path view")
                .expect("header is present")
                .preferred(),
            Ok(ReferrerPolicyValue::StrictOrigin)
        );
        assert_eq!(
            ReferrerPolicy::owned(&table)
                .expect("recognized fast-path owned decode")
                .expect("header is present")
                .preferred(),
            Ok(ReferrerPolicyValue::StrictOrigin)
        );
    }

    #[test]
    fn comma_items_skip_empty_members_and_trim_ows() {
        assert_eq!(
            CommaItems::new(b" , no-referrer,\t, origin , ").collect::<Vec<_>>(),
            [b"no-referrer".as_slice(), b"origin".as_slice()]
        );
        assert_eq!(CommaItems::new(b",,,").next(), None);
    }

    #[test]
    fn malformed_private_storage_surfaces_iteration_errors() {
        let malformed = ReferrerPolicyOwned {
            values: super::FieldLinesIter::one(FieldValue::from_static("origin, bad token")),
        };
        let mut tokens = malformed.tokens();
        assert_eq!(
            tokens.next().expect("first token").expect("recognized token").policy(),
            Some(ReferrerPolicyValue::Origin)
        );
        assert_eq!(
            tokens.next().expect("second token").expect_err("invalid token").kind(),
            DecodeErrorKind::InvalidToken
        );
        assert_eq!(
            malformed.preferred().expect_err("iteration error propagates").kind(),
            DecodeErrorKind::InvalidToken
        );

        let malformed_view = super::ReferrerPolicyView {
            values: crate::source::FieldLines::single(&crate::FieldName::ReferrerPolicy, b"bad token"),
        };
        assert_eq!(
            malformed_view.preferred().expect_err("borrowed iteration error propagates").kind(),
            DecodeErrorKind::InvalidToken
        );
    }
}
