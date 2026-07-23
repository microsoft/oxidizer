// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Adversarial primitive path capture coverage for generated static and dynamic routes.

use std::str::FromStr;

use routerama::resolve::{HttpMethod, ResolveError, resolver};

#[derive(Debug, PartialEq, Eq)]
struct Custom(u8);

impl FromStr for Custom {
    type Err = std::num::ParseIntError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        value.parse().map(Self)
    }
}

#[resolver]
#[derive(Debug, PartialEq, Eq)]
enum StaticPrimitive {
    #[route(GET, "/u8/{value}")]
    U8 { value: u8 },
    #[route(GET, "/u16/{value}")]
    U16 { value: u16 },
    #[route(GET, "/u32/{value}")]
    U32 { value: u32 },
    #[route(GET, "/u64/{value}")]
    U64 { value: u64 },
    #[route(GET, "/u128/{value}")]
    U128 { value: u128 },
    #[route(GET, "/usize/{value}")]
    Usize { value: usize },
    #[route(GET, "/i8/{value}")]
    I8 { value: i8 },
    #[route(GET, "/i16/{value}")]
    I16 { value: i16 },
    #[route(GET, "/i32/{value}")]
    I32 { value: i32 },
    #[route(GET, "/i64/{value}")]
    I64 { value: i64 },
    #[route(GET, "/i128/{value}")]
    I128 { value: i128 },
    #[route(GET, "/isize/{value}")]
    Isize { value: isize },
    #[route(GET, "/bool/{value}")]
    Bool { value: bool },
    #[route(GET, "/custom/{value}")]
    Custom { value: Custom },
}

#[resolver]
#[derive(Debug, PartialEq, Eq)]
enum DynamicPrimitive {
    #[route(dynamic)]
    U128 { value: u128 },
    #[route(dynamic)]
    I128 { value: i128 },
    #[route(dynamic)]
    Bool { value: bool },
    #[route(dynamic)]
    Custom { value: Custom },
}

fn encode(value: &str) -> String {
    value.bytes().fold(String::new(), |mut encoded, byte| {
        use std::fmt::Write as _;

        write!(encoded, "%{byte:02X}").expect("writing to a String succeeds");
        encoded
    })
}

fn static_resolver() -> StaticPrimitiveResolver {
    StaticPrimitive::resolver()
}

fn dynamic_resolver() -> DynamicPrimitiveResolver {
    DynamicPrimitive::builder()
        .add_u128(HttpMethod::GET, "/u128/{value}")
        .add_i128(HttpMethod::GET, "/i128/{value}")
        .add_bool(HttpMethod::GET, "/bool/{value}")
        .add_custom(HttpMethod::GET, "/custom/{value}")
        .build()
        .expect("the primitive dynamic routes are valid")
}

macro_rules! assert_unsigned {
    ($variant:ident, $ty:ty) => {{
        let resolver = static_resolver();
        let zero = format!("/{}/{}", stringify!($variant).to_ascii_lowercase(), encode("0"));
        let plus = format!("/{}/{}", stringify!($variant).to_ascii_lowercase(), encode("+42"));
        let max = format!("/{}/{}", stringify!($variant).to_ascii_lowercase(), encode(&<$ty>::MAX.to_string()));
        let overflow_value = <$ty>::MAX
            .checked_add(1)
            .map_or_else(|| format!("{}0", <$ty>::MAX), |value| value.to_string());
        let overflow = format!("/{}/{}", stringify!($variant).to_ascii_lowercase(), encode(&overflow_value));
        assert_eq!(resolver.resolve("GET", &zero), Ok(StaticPrimitive::$variant { value: 0 }));
        assert_eq!(resolver.resolve("GET", &plus), Ok(StaticPrimitive::$variant { value: 42 }));
        assert_eq!(resolver.resolve("GET", &max), Ok(StaticPrimitive::$variant { value: <$ty>::MAX }));
        assert_eq!(resolver.resolve("GET", &overflow), Err(ResolveError::InvalidCapture("value")));
    }};
}

macro_rules! assert_signed {
    ($variant:ident, $ty:ty) => {{
        let resolver = static_resolver();
        let prefix = stringify!($variant).to_ascii_lowercase();
        let zero = format!("/{prefix}/{}", encode("-0"));
        let plus = format!("/{prefix}/{}", encode("+42"));
        let min = format!("/{prefix}/{}", encode(&<$ty>::MIN.to_string()));
        let max = format!("/{prefix}/{}", encode(&<$ty>::MAX.to_string()));
        let overflow = format!("/{prefix}/{}", encode(&format!("{}0", <$ty>::MAX)));
        let underflow = format!("/{prefix}/{}", encode(&format!("{}0", <$ty>::MIN)));
        assert_eq!(resolver.resolve("GET", &zero), Ok(StaticPrimitive::$variant { value: 0 }));
        assert_eq!(resolver.resolve("GET", &plus), Ok(StaticPrimitive::$variant { value: 42 }));
        assert_eq!(resolver.resolve("GET", &min), Ok(StaticPrimitive::$variant { value: <$ty>::MIN }));
        assert_eq!(resolver.resolve("GET", &max), Ok(StaticPrimitive::$variant { value: <$ty>::MAX }));
        assert_eq!(resolver.resolve("GET", &overflow), Err(ResolveError::InvalidCapture("value")));
        assert_eq!(resolver.resolve("GET", &underflow), Err(ResolveError::InvalidCapture("value")));
    }};
}

#[test]
fn every_unsigned_width_accepts_encoded_boundaries_and_rejects_overflow() {
    assert_unsigned!(U8, u8);
    assert_unsigned!(U16, u16);
    assert_unsigned!(U32, u32);
    assert_unsigned!(U64, u64);
    assert_unsigned!(U128, u128);
    assert_unsigned!(Usize, usize);
}

#[test]
fn every_signed_width_accepts_encoded_boundaries_and_rejects_overflow() {
    assert_signed!(I8, i8);
    assert_signed!(I16, i16);
    assert_signed!(I32, i32);
    assert_signed!(I64, i64);
    assert_signed!(I128, i128);
    assert_signed!(Isize, isize);
}

#[test]
fn encoded_bool_spelling_matches_rust_from_str() {
    let resolver = static_resolver();
    assert_eq!(
        resolver.resolve("GET", &format!("/bool/{}", encode("true"))),
        Ok(StaticPrimitive::Bool { value: true })
    );
    assert_eq!(
        resolver.resolve("GET", &format!("/bool/{}", encode("false"))),
        Ok(StaticPrimitive::Bool { value: false })
    );
    for invalid in ["True", "FALSE", "1", "yes", "+true"] {
        assert_eq!(
            resolver.resolve("GET", &format!("/bool/{}", encode(invalid))),
            Err(ResolveError::InvalidCapture("value")),
            "{invalid:?} must remain invalid for bool"
        );
    }
}

#[test]
fn malformed_escapes_and_invalid_utf8_remain_undecodable() {
    let resolver = static_resolver();
    for suffix in [
        "%",
        "%0",
        "%GG",
        "%FF",
        "%C0%80",
        "%ED%A0%80",
        "%F4%90%80%80",
        "1%",
        "1%FF",
        "256%",
        "999999999999999999999999999999999999999999999999999999999999%",
        "x%",
    ] {
        assert_eq!(
            resolver.resolve("GET", &format!("/u8/{suffix}")),
            Err(ResolveError::UndecodableCapture("value")),
            "{suffix:?} must be classified as undecodable"
        );
    }
    for suffix in ["%C3%A9", "%E2%9C%93", "%F0%9F%A6%80"] {
        assert_eq!(
            resolver.resolve("GET", &format!("/u8/{suffix}")),
            Err(ResolveError::InvalidCapture("value")),
            "valid UTF-8 that is not numeric must be classified as invalid"
        );
    }
}

#[test]
fn boundary_characters_and_custom_from_str_keep_their_contracts() {
    let resolver = static_resolver();
    for suffix in [
        "%2F",
        "%3A",
        "%2B",
        "%2D",
        "%20",
        "%30%2F",
        "1%2F",
        "256%2F",
        "999999999999999999999999999999999999999999999999999999999999%2F",
        "x%C3%A9",
    ] {
        assert_eq!(
            resolver.resolve("GET", &format!("/u8/{suffix}")),
            Err(ResolveError::InvalidCapture("value")),
            "{suffix:?} decodes successfully but is not an unsigned integer"
        );
    }
    assert_eq!(
        resolver.resolve("GET", &format!("/custom/{}", encode("42"))),
        Ok(StaticPrimitive::Custom { value: Custom(42) })
    );
    assert_eq!(
        resolver.resolve("GET", "/custom/42"),
        Ok(StaticPrimitive::Custom { value: Custom(42) })
    );
}

#[test]
fn dynamic_primitive_extractors_share_static_semantics() {
    let resolver = dynamic_resolver();
    assert_eq!(
        resolver.resolve("GET", &format!("/u128/{}", encode(&u128::MAX.to_string()))),
        Ok(DynamicPrimitive::U128 { value: u128::MAX })
    );
    assert_eq!(
        resolver.resolve("GET", &format!("/i128/{}", encode(&i128::MIN.to_string()))),
        Ok(DynamicPrimitive::I128 { value: i128::MIN })
    );
    assert_eq!(
        resolver.resolve("GET", &format!("/bool/{}", encode("false"))),
        Ok(DynamicPrimitive::Bool { value: false })
    );
    assert_eq!(
        resolver.resolve("GET", &format!("/custom/{}", encode("42"))),
        Ok(DynamicPrimitive::Custom { value: Custom(42) })
    );
    assert_eq!(resolver.resolve("GET", "/u128/%FF"), Err(ResolveError::UndecodableCapture("value")));
    assert_eq!(
        resolver.resolve("GET", &format!("/u128/{}", encode(&format!("{}0", u128::MAX)))),
        Err(ResolveError::InvalidCapture("value"))
    );
}
