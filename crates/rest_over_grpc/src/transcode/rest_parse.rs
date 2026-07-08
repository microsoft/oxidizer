// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Typed conversion of a captured path variable's string value into a message
//! field, poked directly by the generated transcoder.
//!
//! [`parse_path_field`] percent-decodes the value and converts it to the field
//! type `T` via [`RestParse`], whose target type is inferred from the field
//! being assigned. The [`RestParse`] impls cover every proto scalar field
//! representation: the string/number/bool types, `bytes` (base64, as `Vec<u8>`
//! or [`bytes::Bytes`]), and `optional` presence (`Option<T>`). Enum fields —
//! which prost represents as a bare `i32`, indistinguishable at the type level
//! from a plain `int32` — are handled separately by [`parse_path_enum_value`],
//! which the generator emits with the concrete enum type so it can accept the
//! enum value *name* as well as its number.

use std::borrow::Cow;

use base64::Engine;
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};

use super::{TranscodeError, percent};

/// Base64 engines accepting padded or unpadded input (proto3 JSON accepts
/// both), one for the standard alphabet and one for the URL-safe alphabet.
const STANDARD: GeneralPurpose = GeneralPurpose::new(
    &base64::alphabet::STANDARD,
    GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent),
);
const URL_SAFE: GeneralPurpose = GeneralPurpose::new(
    &base64::alphabet::URL_SAFE,
    GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent),
);

/// Converts a percent-decoded path/query value into a message field of the
/// implementing type.
///
/// This is the conversion the generated transcoder uses to poke a captured path
/// variable directly into a request-message field: the impl is selected by the
/// field's Rust type, so no per-field type information is needed at
/// code-generation time. Impls exist for the proto scalar field types (`String`,
/// the integer and float types, `bool`), for `bytes` fields (`Vec<u8>` and
/// [`bytes::Bytes`], base64-decoded), and for `optional` presence (`Option<T>`).
pub trait RestParse: Sized {
    /// Converts an already-percent-decoded value into `Self`.
    ///
    /// Takes a [`Cow`] so a `String` field can adopt the decoder's owned buffer
    /// (when percent-decoding produced one) instead of copying it again.
    ///
    /// # Errors
    ///
    /// Returns a [`TranscodeError`] (mapping to
    /// [`Code::InvalidArgument`](crate::handling::Code::InvalidArgument)) if `decoded` is
    /// not a valid representation of `Self`.
    fn parse_rest(decoded: Cow<'_, str>) -> Result<Self, TranscodeError>;
}

macro_rules! impl_rest_parse_from_str {
    ($($ty:ty),+ $(,)?) => {
        $(
            impl RestParse for $ty {
                fn parse_rest(decoded: Cow<'_, str>) -> Result<Self, TranscodeError> {
                    let decoded = decoded.as_ref();
                    decoded.parse::<$ty>().map_err(|error| TranscodeError::path_field(decoded, &error))
                }
            }
        )+
    };
}

// The proto scalar field types whose prost representation parses from a string:
// `bool` and every integer/float width proto3 produces. `String` is handled
// separately (below) so it can consume the decoded buffer without a re-copy.
impl_rest_parse_from_str!(bool, i32, i64, u32, u64, f32, f64);

impl RestParse for String {
    fn parse_rest(decoded: Cow<'_, str>) -> Result<Self, TranscodeError> {
        // Adopt the buffer percent-decoding already allocated (`Cow::Owned`)
        // rather than copying it a second time via `str::parse::<String>()`; a
        // borrowed value still allocates exactly once here.
        Ok(decoded.into_owned())
    }
}

impl RestParse for Vec<u8> {
    fn parse_rest(decoded: Cow<'_, str>) -> Result<Self, TranscodeError> {
        decode_base64(decoded.as_ref())
    }
}

impl RestParse for bytes::Bytes {
    fn parse_rest(decoded: Cow<'_, str>) -> Result<Self, TranscodeError> {
        decode_base64(decoded.as_ref()).map(Self::from)
    }
}

impl<T: RestParse> RestParse for Option<T> {
    fn parse_rest(decoded: Cow<'_, str>) -> Result<Self, TranscodeError> {
        T::parse_rest(decoded).map(Some)
    }
}

/// Decodes a base64 `bytes` field value, accepting the standard or URL-safe
/// alphabet with or without padding (matching proto3 JSON).
fn decode_base64(value: &str) -> Result<Vec<u8>, TranscodeError> {
    STANDARD
        .decode(value)
        .or_else(|_| URL_SAFE.decode(value))
        .map_err(|error| TranscodeError::path_field(value, &error))
}

/// Parses a captured path-variable value into a typed message field.
///
/// The value is percent-decoded (path style — `%XX` is decoded, `+` stays
/// literal) and converted to the field type `T` via [`RestParse`]. Because the
/// generated code assigns the result straight into the message field, `T` is
/// inferred from that field, so no per-field type information is needed at
/// code-generation time.
///
/// # Errors
///
/// Returns a [`TranscodeError`] (mapping to
/// [`Code::InvalidArgument`](crate::handling::Code::InvalidArgument)) if the decoded value
/// does not parse as `T`.
///
/// # Examples
///
/// ```
/// use rest_over_grpc::codegen_helpers::parse_path_field;
///
/// // A `String` field: percent-decoded, never fails.
/// let name: String = parse_path_field("shelves%2F7")?;
/// assert_eq!(name, "shelves/7");
///
/// // A numeric field: parsed from the (quoted-in-the-path) value.
/// let book: i64 = parse_path_field("1347")?;
/// assert_eq!(book, 1347);
///
/// // An `optional` field: presence is added automatically.
/// let opt: Option<i32> = parse_path_field("7")?;
/// assert_eq!(opt, Some(7));
/// # Ok::<(), rest_over_grpc::codegen_helpers::TranscodeError>(())
/// ```
pub fn parse_path_field<T: RestParse>(raw: &str) -> Result<T, TranscodeError> {
    let decoded = percent::decode_path(raw);
    T::parse_rest(decoded)
}

/// Parses a captured path-variable value into an enum field's `i32` value,
/// accepting either the enum value's *number* or its *name*.
///
/// Enum fields are a bare `i32` in prost, so — unlike the scalar/bytes/optional
/// cases handled by [`parse_path_field`] — the concrete enum type cannot be
/// inferred from the field. The generator therefore emits this call with the
/// enum's `from_str_name` associated function, letting the value be given by
/// name (e.g. `ACTIVE`) as well as by number, matching the proto3 JSON mapping.
/// The returned `i32` is assigned into the field (via `.into()`, which also
/// wraps an `optional` enum field's `Option<i32>`).
///
/// # Errors
///
/// Returns a [`TranscodeError`] (mapping to
/// [`Code::InvalidArgument`](crate::handling::Code::InvalidArgument)) if the decoded value
/// is neither a number nor a known enum value name.
///
/// # Examples
///
/// ```
/// use rest_over_grpc::codegen_helpers::parse_path_enum_value;
///
/// // Stand-in for a prost-generated enum: `from_str_name` maps a name to a
/// // variant, and the variant converts into its `i32` number.
/// #[derive(Clone, Copy)]
/// enum State {
///     Active,
/// }
/// impl From<State> for i32 {
///     fn from(_: State) -> i32 {
///         1
///     }
/// }
/// fn from_str_name(name: &str) -> Option<State> {
///     (name == "ACTIVE").then_some(State::Active)
/// }
///
/// assert_eq!(parse_path_enum_value("ACTIVE", from_str_name)?, 1); // by name
/// assert_eq!(parse_path_enum_value("2", from_str_name)?, 2); // by number
///
/// # Ok::<(), rest_over_grpc::codegen_helpers::TranscodeError>(())
/// ```
pub fn parse_path_enum_value<E: Into<i32>>(raw: &str, from_name: fn(&str) -> Option<E>) -> Result<i32, TranscodeError> {
    let decoded = percent::decode_path(raw);
    let value = decoded.as_ref();
    if let Ok(number) = value.parse::<i32>() {
        return Ok(number);
    }
    match from_name(value) {
        Some(variant) => Ok(variant.into()),
        None => Err(TranscodeError::path_enum(value)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalar_and_optional_fields_parse() {
        let name: String = parse_path_field("shelves%2F7").expect("string");
        assert_eq!(name, "shelves/7");
        let n: i64 = parse_path_field("1347").expect("i64");
        assert_eq!(n, 1347);
        let opt: Option<i32> = parse_path_field("7").expect("optional");
        assert_eq!(opt, Some(7));
        let bad = parse_path_field::<i32>("nope").expect_err("bad i32");
        assert_eq!(bad.code(), crate::handling::Code::InvalidArgument);
    }

    #[test]
    fn bytes_fields_base64_decode() {
        // "aGk=" is base64 for "hi"; also accept the unpadded / URL-safe forms.
        let v: Vec<u8> = parse_path_field("aGk%3D").expect("padded base64 (%3D = '=')");
        assert_eq!(v, b"hi");
        let v: Vec<u8> = parse_path_field("aGk").expect("unpadded base64");
        assert_eq!(v, b"hi");
        let b: bytes::Bytes = parse_path_field("aGk=").expect("bytes::Bytes");
        assert_eq!(b.as_ref(), b"hi");
        let bad = parse_path_field::<Vec<u8>>("!!!!").expect_err("invalid base64");
        assert_eq!(bad.code(), crate::handling::Code::InvalidArgument);
    }

    #[test]
    fn enum_value_by_name_or_number() {
        #[derive(Clone, Copy)]
        enum State {
            Active,
        }
        impl From<State> for i32 {
            fn from(_: State) -> Self {
                1
            }
        }
        fn from_str_name(name: &str) -> Option<State> {
            (name == "ACTIVE").then_some(State::Active)
        }

        assert_eq!(parse_path_enum_value("ACTIVE", from_str_name).expect("by name"), 1);
        assert_eq!(parse_path_enum_value("2", from_str_name).expect("by number"), 2);
        let bad = parse_path_enum_value("BOGUS", from_str_name).expect_err("unknown name");
        assert_eq!(bad.code(), crate::handling::Code::InvalidArgument);
    }
}
