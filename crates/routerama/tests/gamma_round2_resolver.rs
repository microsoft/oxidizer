// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Static, dynamic, and mixed generated resolver branches.

use routerama::{HttpMethod, ResolveError, resolver};

#[resolver]
#[derive(Debug, PartialEq)]
enum Round2StaticOnly {
    #[route(GET, "/ready")]
    Ready,
}

#[resolver]
#[derive(Debug, PartialEq)]
enum Round2Empty {}

#[resolver]
#[derive(Debug, PartialEq)]
enum Round2DynamicOnly {
    Unit,
    Number { r#type: u32 },
}

#[resolver]
#[derive(Debug, PartialEq)]
enum Round2Mixed<'p> {
    #[route(GET, "/verb/{value}:lookup")]
    Verb {
        value: &'p str,
    },
    #[route(GET, "/plain/{value}")]
    Plain {
        value: &'p str,
    },
    #[route(GET, "/health")]
    Health,
    Dynamic {
        value: String,
    },
}

#[test]
fn gamma_round2_generated_static_only_resolver_has_no_synthetic_lifetime() {
    let resolver = Round2StaticOnly::resolver();
    assert_eq!(resolver.resolve("GET", "/ready"), Ok(Round2StaticOnly::Ready));
    assert_eq!(resolver.resolve("POST", "/ready"), Err(ResolveError::NotFound("/ready")));
    assert_eq!(resolver.resolve("GET", "/absent"), Err(ResolveError::NotFound("/absent")));
    assert_eq!(
        Round2Empty::resolver().resolve("GET", "/anything"),
        Err(ResolveError::NotFound("/anything"))
    );
}

#[test]
fn gamma_round2_generated_dynamic_only_resolver_preserves_raw_field_error_keys() {
    let resolver = Round2DynamicOnly::builder()
        .add_unit(HttpMethod::GET, "/unit")
        .add_number(HttpMethod::GET, "/number/{type}")
        .build()
        .expect("every dynamic variant has a valid registration");
    assert_eq!(resolver.resolve("GET", "/unit"), Ok(Round2DynamicOnly::Unit));
    assert_eq!(resolver.resolve("GET", "/number/41"), Ok(Round2DynamicOnly::Number { r#type: 41 }));
    assert_eq!(resolver.resolve("GET", "/number/bad"), Err(ResolveError::InvalidCapture("type")));
    assert_eq!(resolver.resolve("GET", "/missing"), Err(ResolveError::NotFound("/missing")));
}

#[test]
fn gamma_round2_generated_mixed_resolver_retains_earlier_verbs_and_borrowed_static_values() {
    let resolver = Round2Mixed::builder()
        .add_dynamic(HttpMethod::GET, "/dynamic/{value}")
        .build()
        .expect("the dynamic registration complements the two static variants");
    let path = String::from("/verb/value:lookup");
    let matched = resolver.resolve("GET", &path).expect("the first static route retains its verb");
    let Round2Mixed::Verb { value } = matched else {
        panic!("the verb route must select its own static variant");
    };
    assert_eq!(value, "value");
    assert_eq!(value.as_ptr(), path.as_ptr().wrapping_add(6));
    assert_eq!(resolver.resolve("GET", "/plain/raw"), Ok(Round2Mixed::Plain { value: "raw" }));
    assert_eq!(resolver.resolve("GET", "/health"), Ok(Round2Mixed::Health));
    assert_eq!(
        resolver.resolve("GET", "/dynamic/a%20b"),
        Ok(Round2Mixed::Dynamic {
            value: String::from("a b")
        })
    );
    assert_eq!(
        resolver.resolve("GET", "/dynamic/a:unknown"),
        Err(ResolveError::NotFound("/dynamic/a:unknown"))
    );
    assert_eq!(
        resolver.resolve("GET", "/verb/value:wrong"),
        Err(ResolveError::NotFound("/verb/value:wrong"))
    );
}
