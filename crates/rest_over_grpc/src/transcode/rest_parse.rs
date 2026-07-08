// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Converts captured path values into protobuf field types.
//!
//! [`RestParse`] covers scalar, bytes, and optional fields. Enum helpers accept
//! either the protobuf name or numeric value.

use std::borrow::Cow;

use base64::Engine;
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
use bytes::Bytes;

use super::{TranscodeError, percent};

/// Proto3 JSON accepts both base64 alphabets with optional padding.
const STANDARD: GeneralPurpose = GeneralPurpose::new(
    &base64::alphabet::STANDARD,
    GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent),
);
const URL_SAFE: GeneralPurpose = GeneralPurpose::new(
    &base64::alphabet::URL_SAFE,
    GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent),
);

/// Converts a decoded path value into a protobuf field.
pub trait RestParse: Sized {
    /// Converts an already-percent-decoded value into `Self`.
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

impl_rest_parse_from_str!(bool, i32, i64, u32, u64);

macro_rules! impl_rest_parse_float {
    ($ty:ty) => {
        impl RestParse for $ty {
            fn parse_rest(decoded: Cow<'_, str>) -> Result<Self, TranscodeError> {
                let value = decoded.as_ref();
                match value {
                    "NaN" => Ok(<$ty>::NAN),
                    "Infinity" => Ok(<$ty>::INFINITY),
                    "-Infinity" => Ok(<$ty>::NEG_INFINITY),
                    _ => {
                        let parsed = serde_json::from_str::<$ty>(value).map_err(|error| TranscodeError::path_field(value, &error))?;
                        if parsed.is_finite() {
                            Ok(parsed)
                        } else {
                            Err(TranscodeError::path_field(value, &"float is out of range"))
                        }
                    }
                }
            }
        }
    };
}

impl_rest_parse_float!(f32);
impl_rest_parse_float!(f64);

impl RestParse for String {
    fn parse_rest(decoded: Cow<'_, str>) -> Result<Self, TranscodeError> {
        Ok(decoded.into_owned())
    }
}

impl RestParse for Vec<u8> {
    fn parse_rest(decoded: Cow<'_, str>) -> Result<Self, TranscodeError> {
        decode_base64(decoded.as_ref())
    }
}

impl RestParse for Bytes {
    fn parse_rest(decoded: Cow<'_, str>) -> Result<Self, TranscodeError> {
        decode_base64(decoded.as_ref()).map(Self::from)
    }
}

impl<T: RestParse> RestParse for Option<T> {
    fn parse_rest(decoded: Cow<'_, str>) -> Result<Self, TranscodeError> {
        T::parse_rest(decoded).map(Some)
    }
}

fn decode_base64(value: &str) -> Result<Vec<u8>, TranscodeError> {
    STANDARD
        .decode(value)
        .or_else(|_| URL_SAFE.decode(value))
        .map_err(|error| TranscodeError::path_field(value, &error))
}

/// Parses a captured path-variable value into a typed message field.
///
/// The generated assignment infers `T` from the destination field.
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
/// // A `String` field: percent-decoded (malformed encoding is rejected).
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
    let decoded = percent::decode_path(raw).ok_or_else(|| TranscodeError::invalid_encoding("path variable"))?;
    T::parse_rest(decoded)
}

/// Parses a multi-segment path variable using the default
/// `google.api.http` reserved-expansion decoding rules.
///
/// # Errors
///
/// Returns [`TranscodeError`] for malformed percent encoding, invalid UTF-8,
/// or a value that does not parse as `T`.
pub fn parse_reserved_path_field<T: RestParse>(raw: &str) -> Result<T, TranscodeError> {
    let decoded = percent::decode_reserved_path(raw).ok_or_else(|| TranscodeError::invalid_encoding("path variable"))?;
    T::parse_rest(decoded)
}

/// Parses a captured path-variable value into an enum field's `i32` value,
/// accepting either the enum value's *number* or its *name*.
///
/// Prost stores enums as `i32`, so generated code supplies the enum's
/// `from_str_name` function.
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
    let decoded = percent::decode_path(raw).ok_or_else(|| TranscodeError::invalid_encoding("path variable"))?;
    let value = decoded.as_ref();
    if let Ok(number) = value.parse::<i32>() {
        return Ok(number);
    }
    match from_name(value) {
        Some(variant) => Ok(variant.into()),
        None => Err(TranscodeError::path_enum(value)),
    }
}

/// Parses a multi-segment enum path variable using the default
/// `google.api.http` reserved-expansion decoding rules.
///
/// # Errors
///
/// Returns [`TranscodeError`] for malformed percent encoding, invalid UTF-8,
/// or a value that is neither a valid enum name nor an `i32`.
pub fn parse_reserved_path_enum_value<E: Into<i32>>(raw: &str, from_name: fn(&str) -> Option<E>) -> Result<i32, TranscodeError> {
    let decoded = percent::decode_reserved_path(raw).ok_or_else(|| TranscodeError::invalid_encoding("path variable"))?;
    let value = decoded.as_ref();
    if let Ok(number) = value.parse::<i32>() {
        return Ok(number);
    }
    from_name(value).map(Into::into).ok_or_else(|| TranscodeError::path_enum(value))
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
    fn float_fields_follow_proto_json_tokens() {
        assert!((parse_path_field::<f64>("1.5").expect("finite") - 1.5).abs() < f64::EPSILON);
        assert!(parse_path_field::<f64>("NaN").expect("NaN").is_nan());
        let infinity = parse_path_field::<f64>("Infinity").expect("infinity");
        assert!(infinity.is_infinite() && infinity.is_sign_positive());
        let _ = parse_path_field::<f64>("inf").expect_err("non-canonical infinity");
        let _ = parse_path_field::<f64>("+1.0").expect_err("leading plus");
    }

    #[test]
    fn bytes_fields_base64_decode() {
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

    #[test]
    fn path_fields_reject_bad_encoding_and_reserved_expansions_keep_slashes() {
        let _ = parse_path_field::<String>("%FF").expect_err("invalid UTF-8");
        let _ = parse_path_field::<String>("%zz").expect_err("malformed escape");
        assert_eq!(
            parse_reserved_path_field::<String>("shelves%2F7").expect("reserved expansion"),
            "shelves%2F7"
        );
    }
}
