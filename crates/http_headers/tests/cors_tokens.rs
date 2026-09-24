// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Shared field-name and method tokens remain available with only CORS enabled.

#![cfg(feature = "headers-cors")]

use std::collections::HashSet;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use http_headers::headers::{
    AccessControlAllowHeaders, AccessControlAllowMethods, AccessControlExposeHeaders, AccessControlRequestHeaders,
    AccessControlRequestMethod, FieldNameView, InvalidMethod, MethodView,
};
#[cfg(feature = "http")]
use http_headers::headers::{
    AccessControlAllowHeadersOwned, AccessControlAllowMethodsOwned, AccessControlExposeHeadersOwned, AccessControlRequestHeadersOwned,
    AccessControlRequestMethodOwned,
};
use http_headers::source::{FieldLines, FieldSource};
use http_headers::{FieldName, FieldValue};

struct Source {
    name: &'static FieldName,
    values: Vec<FieldValue>,
}

impl FieldSource for Source {
    fn lines(&self, name: &'static FieldName) -> Option<FieldLines<'_>> {
        (name == self.name).then(|| FieldLines::from_slice(name, &self.values)).flatten()
    }
}

fn hash(value: &impl Hash) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

#[test]
fn cors_names_share_case_insensitive_identity_without_changing_wire_spelling() {
    macro_rules! check {
        ($header:ty, $name:expr) => {{
            let source = Source {
                name: $name,
                values: vec![FieldValue::from_static("X-Trace, content-type"), FieldValue::from_static("x-trace")],
            };
            let owned = <$header>::owned(&source).unwrap().unwrap();
            let borrowed = <$header>::view(&source).unwrap().unwrap();
            let owned_names: Vec<FieldNameView<'_>> = owned.iter().collect();
            let borrowed_names: Vec<FieldNameView<'_>> = borrowed.iter().collect();
            assert_eq!(owned_names, borrowed_names);
            assert_eq!(
                borrowed_names.iter().map(|name| name.as_str()).collect::<Vec<_>>(),
                ["X-Trace", "content-type", "x-trace"]
            );
            assert_eq!(
                owned_names.iter().map(|name| name.as_str()).collect::<Vec<_>>(),
                ["X-Trace", "content-type", "x-trace"]
            );
            assert_eq!(borrowed_names[0], borrowed_names[2]);
            assert_eq!(borrowed_names[0], FieldNameView::new("x-trace").unwrap());
            assert_eq!(hash(&borrowed_names[0]), hash(&borrowed_names[2]));
            assert_eq!(borrowed_names.iter().copied().collect::<HashSet<_>>().len(), 2);
            assert_eq!(borrowed_names[0].as_bytes(), b"X-Trace");
            assert_eq!(borrowed_names[0].try_to_field_name().unwrap().as_str(), "x-trace");
            assert_eq!(borrowed_names[1].try_to_field_name().unwrap(), FieldName::ContentType);
            assert_eq!(
                owned.field_values().map(|value| value.as_bytes()).collect::<Vec<_>>(),
                [b"X-Trace, content-type".as_slice(), b"x-trace"]
            );
        }};
    }
    check!(AccessControlAllowHeaders, &FieldName::AccessControlAllowHeaders);
    check!(AccessControlExposeHeaders, &FieldName::AccessControlExposeHeaders);
    check!(AccessControlRequestHeaders, &FieldName::AccessControlRequestHeaders);
}

#[test]
fn cors_methods_share_case_sensitive_identity_and_extension_tokens() {
    let source = Source {
        name: &FieldName::AccessControlAllowMethods,
        values: vec![FieldValue::from_static("GET, get, X-CUSTOM, *"), FieldValue::from_static("GET")],
    };
    let borrowed = AccessControlAllowMethods::view(&source).unwrap().unwrap();
    let owned = AccessControlAllowMethods::owned(&source).unwrap().unwrap();
    let methods: Vec<MethodView<'_>> = borrowed.iter().collect();
    assert_eq!(methods, owned.iter().collect::<Vec<_>>());
    assert_eq!(
        methods.iter().map(|method| method.as_str()).collect::<Vec<_>>(),
        ["GET", "get", "X-CUSTOM", "*", "GET"]
    );
    assert_eq!(
        owned.iter().map(MethodView::as_str).collect::<Vec<_>>(),
        ["GET", "get", "X-CUSTOM", "*", "GET"]
    );
    assert_eq!(methods[0], MethodView::GET);
    assert_ne!(methods[0], methods[1]);
    assert_eq!(methods[2], MethodView::new("X-CUSTOM").unwrap());
    assert_ne!(methods[2], MethodView::new("x-custom").unwrap());
    assert_eq!(methods.iter().copied().collect::<HashSet<_>>().len(), 4);
    assert_eq!(hash(&methods[0]), hash(&methods[4]));
    assert_eq!(MethodView::new("bad method").unwrap_err(), InvalidMethod);

    for wire in ["GET", "get", "X-CUSTOM", "*"] {
        let source = Source {
            name: &FieldName::AccessControlRequestMethod,
            values: vec![FieldValue::from_str(wire).unwrap()],
        };
        let borrowed = AccessControlRequestMethod::view(&source).unwrap().unwrap();
        let owned = AccessControlRequestMethod::owned(&source).unwrap().unwrap();
        let method = borrowed.method();
        assert_eq!(method, owned.method().unwrap());
        assert_eq!(method, MethodView::new(wire).unwrap());
        assert_eq!(method.as_bytes(), wire.as_bytes());
    }
}

#[cfg(feature = "http")]
#[test]
fn shared_cors_tokens_convert_to_http_names_and_methods() {
    let allowed = AccessControlAllowHeadersOwned::from_header_names(["Content-Type", "X-Trace"]).unwrap();
    let exposed = AccessControlExposeHeadersOwned::from_header_names(["Content-Type", "X-Trace"]).unwrap();
    let requested = AccessControlRequestHeadersOwned::from_header_names(["Content-Type", "X-Trace"]).unwrap();
    for names in [
        allowed.iter().collect::<Vec<_>>(),
        exposed.iter().collect(),
        requested.iter().collect(),
    ] {
        assert_eq!(names[0].try_to_http_header_name().unwrap(), http::header::CONTENT_TYPE);
        assert_eq!(names[1].try_to_http_header_name().unwrap().as_str(), "x-trace");
        assert_eq!(names[1].as_str(), "X-Trace");
    }
    let allowed = AccessControlAllowMethodsOwned::from_methods(["GET", "get", "X-CUSTOM", "*"]).unwrap();
    for method in &allowed {
        assert_eq!(method.try_to_method().unwrap().as_str(), method.as_str());
        let requested = AccessControlRequestMethodOwned::from_method(method.as_str()).unwrap();
        assert_eq!(
            requested.method().unwrap().try_to_method().unwrap(),
            method.try_to_method().unwrap()
        );
    }
}
