// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Sensitive Host diagnostics redact the complete authority.

#![cfg(feature = "headers-negotiation")]

use std::fmt::{self, Write as _};

use http_headers::headers::{Host, HostKind};
use http_headers::{DecodeMode, FieldSensitivity, FieldValue, SingleValueField};

fn assert_redacted(value: &impl fmt::Debug, name: &str, field_type: &str) {
    assert_eq!(format!("{value:?}"), format!("{name} {{ value: {field_type}(Sensitive), .. }}"));
    assert_eq!(
        format!("{value:#?}"),
        format!("{name} {{\n    value: {field_type}(Sensitive),\n    ..\n}}")
    );
}

#[test]
fn sensitive_authorities_redact_all_owned_and_borrowed_debug_fields() {
    for (wire, host, mode) in [
        ("internal.example:08443", "internal.example", DecodeMode::Strict),
        ("192.0.2.123:08443", "192.0.2.123", DecodeMode::Strict),
        ("[2001:0DB8::123]:08443", "[2001:0DB8::123]", DecodeMode::Strict),
        ("[vF.internal:node]:08443", "[vF.internal:node]", DecodeMode::Strict),
        ("münich.example:08443", "münich.example", DecodeMode::Relaxed),
    ] {
        let value = FieldValue::from_str(wire).unwrap().with_sensitivity(FieldSensitivity::Sensitive);
        let borrowed = Host::decode_view_with(value.as_field_value_ref(), mode).unwrap();
        let owned = Host::decode_owned_with(value.clone(), mode).unwrap();
        let owned_view = owned.as_view();

        assert_redacted(&owned, "HostOwned", "FieldValue");
        for view in [&borrowed, &owned_view] {
            assert_redacted(view, "HostView", "FieldValueRef");
            assert_eq!(view.host(), host);
            assert_eq!(view.port(), Some("08443"));
            assert_eq!(view.network_port(), Ok(Some(8443)));
            assert_eq!(view.as_str().unwrap(), wire);
            assert_eq!(view.as_field_value().as_bytes(), wire.as_bytes());
            assert!(view.as_field_value().is_sensitive());
        }
        assert_eq!(owned.host().unwrap(), host);
        assert_eq!(owned.port().unwrap(), Some("08443"));
        assert_eq!(owned.network_port(), Ok(Some(8443)));
        assert_eq!(owned.as_str().unwrap(), wire);
        assert_eq!(owned.kind(), borrowed.kind());
        assert_eq!(owned_view.kind(), borrowed.kind());
        assert_eq!(owned.as_field_value().as_bytes(), wire.as_bytes());
        assert!(owned.as_field_value().is_sensitive());
        if mode == DecodeMode::Relaxed {
            let HostKind::RegisteredName(name) = borrowed.kind() else {
                panic!("the international host must remain a registered name");
            };
            assert_eq!(name.normalized(), "xn--mnich-kva.example");
        }
    }
}

#[test]
fn nonsensitive_diagnostics_retain_host_port_address_and_normalization_details() {
    for (wire, host, kind, mode) in [
        ("internal.example:08443", "internal.example", "RegisteredName", DecodeMode::Strict),
        ("192.0.2.123:08443", "192.0.2.123", "Ipv4(192.0.2.123)", DecodeMode::Strict),
        (
            "[2001:0DB8::123]:08443",
            "[2001:0DB8::123]",
            "Ipv6(2001:db8::123)",
            DecodeMode::Strict,
        ),
        ("münich.example:08443", "münich.example", "RegisteredName", DecodeMode::Relaxed),
    ] {
        let value = FieldValue::from_str(wire).unwrap();
        let borrowed = Host::decode_view_with(value.as_field_value_ref(), mode).unwrap();
        let owned = Host::decode_owned_with(value.clone(), mode).unwrap();
        let owned_view = owned.as_view();
        let owned_debug = format!("{owned:?}");
        assert!(owned_debug.contains(&format!("value: {value:?}")));
        assert!(owned_debug.contains("parsed: ParsedHost"));
        assert!(owned_debug.contains(&format!("kind: {kind}")));
        assert!(owned_debug.contains("numeric_port: Some(Ok(8443))"));
        for view in [&borrowed, &owned_view] {
            let debug = format!("{view:?}");
            assert!(debug.contains(&format!("host: {host:?}")));
            assert!(debug.contains("port: Some(\"08443\")"));
            assert!(debug.contains(&format!("kind: {kind}")));
            assert!(debug.contains("numeric_port: Some(Ok(8443))"));
            assert!(!debug.contains("Sensitive"));
            if mode == DecodeMode::Relaxed {
                assert!(debug.contains("normalized: Some(\"xn--mnich-kva.example\")"));
            }
        }
        assert!(!owned_debug.contains("Sensitive"));
        if mode == DecodeMode::Relaxed {
            assert!(owned_debug.contains("normalized: Some(\"xn--mnich-kva.example\")"));
        }
    }
}

struct RejectWriter;

impl fmt::Write for RejectWriter {
    fn write_str(&mut self, _value: &str) -> fmt::Result {
        Err(fmt::Error)
    }
}

#[test]
fn debug_propagates_formatter_errors_with_and_without_redaction() {
    for sensitivity in [FieldSensitivity::Sensitive, FieldSensitivity::NonSensitive] {
        let value = FieldValue::from_static("internal.example:8443").with_sensitivity(sensitivity);
        let borrowed = Host::decode_view(value.as_field_value_ref()).unwrap();
        let owned = Host::decode_owned(value.clone()).unwrap();
        let owned_view = owned.as_view();
        assert_eq!(write!(&mut RejectWriter, "{owned:?}"), Err(fmt::Error));
        assert_eq!(write!(&mut RejectWriter, "{owned:#?}"), Err(fmt::Error));
        for view in [&borrowed, &owned_view] {
            assert_eq!(write!(&mut RejectWriter, "{view:?}"), Err(fmt::Error));
            assert_eq!(write!(&mut RejectWriter, "{view:#?}"), Err(fmt::Error));
        }
    }
}
