// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Retained Host and CORS-origin components without the external HTTP adapter.

#![cfg(any(feature = "headers-negotiation", feature = "headers-cors"))]

use http_headers::sink::{EncodedValues, FieldSink, InsertError};
use http_headers::source::{FieldLines, FieldSource, MAX_CUSTOM_FIELD_BYTES};
use http_headers::{DecodeErrorKind, DecodeMode, Field, FieldName, FieldValue};
#[cfg(feature = "headers-negotiation")]
use http_headers::{FieldValueRef, SingleValueField};

struct Values {
    name: &'static FieldName,
    values: Vec<FieldValue>,
}

impl Values {
    #[expect(clippy::unwrap_used, reason = "test fixtures must contain valid field values")]
    fn new(name: &'static FieldName, wire: &str) -> Self {
        Self {
            name,
            values: vec![FieldValue::try_from(wire).unwrap()],
        }
    }
}

impl FieldSource for Values {
    fn lines(&self, name: &'static FieldName) -> Option<FieldLines<'_>> {
        (name == self.name).then(|| FieldLines::from_slice(name, &self.values)).flatten()
    }
}

impl FieldSink for Values {
    fn set_values(&mut self, name: &'static FieldName, values: EncodedValues) -> Result<(), InsertError> {
        self.name = name;
        self.values = values.into_iter().collect();
        Ok(())
    }

    fn append_values(&mut self, name: &'static FieldName, values: EncodedValues) -> Result<(), InsertError> {
        assert_eq!(name, self.name);
        self.values.extend(values);
        Ok(())
    }

    fn remove_values(&mut self, name: &'static FieldName) {
        assert_eq!(name, self.name);
        self.values.clear();
    }
}

#[cfg(feature = "headers-negotiation")]
mod host {
    use std::error::Error;
    use std::net::{Ipv4Addr, Ipv6Addr};

    use http_headers::headers::{Host, HostKind, HostOwned, HostPortView, PortConversionError, PortConversionErrorKind};

    use super::*;

    #[test]
    fn empty_host_is_valid_across_borrowed_owned_and_source_paths() {
        let view = <Host as SingleValueField>::decode_view(FieldValueRef::new(b"")).unwrap();
        assert_eq!(view.host(), "");
        assert_eq!(view.port(), None);

        let owned = HostOwned::try_from("").unwrap();
        assert_eq!(owned.host().unwrap(), "");
        assert_eq!(owned.port().unwrap(), None);

        let source = Values::new(&FieldName::Host, "");
        let sourced = Host::view(&source).unwrap().unwrap();
        assert_eq!(sourced.host(), "");
        assert_eq!(sourced.port(), None);

        let parsed_error = HostOwned::try_from(":443").unwrap_err().kind();
        assert_eq!(parsed_error, DecodeErrorKind::InvalidSyntax);
        assert_eq!(HostOwned::with_port("", 443).unwrap_err().kind(), parsed_error);
        assert_eq!(
            HostOwned::from_parts(view.kind(), Some(HostPortView::new("443").unwrap()))
                .unwrap_err()
                .kind(),
            parsed_error
        );
    }

    #[test]
    fn ascii_names_borrow_and_ipv4_classification_does_not_coerce_registered_names() {
        for wire in [
            "Example.COM",
            "exa_mple~host",
            "example%20host",
            "!$&'()*+,;=",
            "127.1",
            "192.168.001.1",
            "256.0.0.1",
            "127.0.0.1.",
            "4294967295",
        ] {
            let view = <Host as SingleValueField>::decode_view(FieldValueRef::new(wire.as_bytes())).unwrap();
            let owned = HostOwned::try_from(wire).unwrap();
            let HostKind::RegisteredName(name) = view.kind() else {
                panic!("registered name was coerced: {wire}");
            };
            assert_eq!(name.as_str(), wire);
            assert_eq!(name.to_string(), wire);
            assert_eq!(name.normalized(), wire);
            assert_eq!(name.as_str().as_ptr(), wire.as_ptr());
            assert_eq!(name.normalized().as_ptr(), wire.as_ptr());
            assert_eq!(owned.kind(), view.kind());
            assert_eq!(owned.as_view().kind(), view.kind());
            assert_eq!(HostOwned::from_parts(view.kind(), None).unwrap(), owned);
        }

        for (wire, address) in [("0.0.0.0", Ipv4Addr::UNSPECIFIED), ("192.0.2.1", Ipv4Addr::new(192, 0, 2, 1))] {
            let owned = HostOwned::try_from(wire).unwrap();
            assert_eq!(owned.kind(), HostKind::Ipv4(address));
            assert_eq!(owned.as_view().kind(), HostKind::Ipv4(address));
            assert_eq!(HostOwned::from_ipv4(address, None), owned);
        }
    }

    #[test]
    fn retained_ipv6_and_ipvfuture_preserve_brackets_and_unbounded_versions() {
        let wire = "[2001:0DB8:0:0:0:0:0:1]:000443";
        let address = Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1);
        let owned = HostOwned::try_from(wire).unwrap();
        let view = <Host as SingleValueField>::decode_view(FieldValueRef::new(wire.as_bytes())).unwrap();
        assert_eq!(owned.kind(), HostKind::Ipv6(address));
        assert_eq!(view.kind(), owned.kind());
        assert_eq!(view.host(), "[2001:0DB8:0:0:0:0:0:1]");
        assert_eq!(view.port(), Some("000443"));
        assert_eq!(view.network_port(), Ok(Some(443)));
        assert_eq!(view.as_field_value().as_bytes(), wire.as_bytes());
        assert_eq!(HostOwned::from_ipv6(address, Some(443)).as_str().unwrap(), "[2001:db8::1]:443");

        let wire = "[VFFFFFFFFFFFFFFFFFFFFFFFFFFFF.alpha:beta!$&'()*+,;=_~]:65536";
        let owned = HostOwned::try_from(wire).unwrap();
        let view = owned.as_view();
        let HostKind::IpvFuture(future) = view.kind() else {
            panic!("IPvFuture was not retained");
        };
        assert_eq!(future.version(), "FFFFFFFFFFFFFFFFFFFFFFFFFFFF");
        assert_eq!(future.address(), "alpha:beta!$&'()*+,;=_~");
        assert_eq!(owned.kind(), view.kind());
        let rebuilt = HostOwned::from_parts(view.kind(), view.port_view()).unwrap();
        assert_eq!(rebuilt.as_str().unwrap(), wire.replacen('V', "v", 1));
        assert_eq!(rebuilt.network_port().unwrap_err().kind(), PortConversionErrorKind::Overflow);
        assert_eq!(owned.as_field_value().as_bytes(), wire.as_bytes());
    }

    #[test]
    fn ipvfuture_dispatch_preserves_components_and_literal_errors() {
        for (literal, version, address) in [
            ("[v1.a]", "1", "a"),
            ("[VfF.a:!$&'()*+,;=_~]", "fF", "a:!$&'()*+,;=_~"),
            ("[v000000000000000000000000FFFFFFFF.a]", "000000000000000000000000FFFFFFFF", "a"),
        ] {
            let wire = format!("{literal}:000443");
            let owned = HostOwned::try_from(wire.as_str()).unwrap();
            let view = <Host as SingleValueField>::decode_view(FieldValueRef::new(wire.as_bytes())).unwrap();
            let HostKind::IpvFuture(future) = view.kind() else {
                panic!("validated IPvFuture was not retained");
            };
            assert_eq!(future.version(), version);
            assert_eq!(future.address(), address);
            assert_eq!(owned.kind(), view.kind());
            assert_eq!(view.host(), literal);
            assert_eq!(owned.host().unwrap(), literal);
            assert_eq!(view.network_port(), Ok(Some(443)));
            assert_eq!(owned.network_port(), Ok(Some(443)));
            assert_eq!(view.port(), Some("000443"));
            assert_eq!(owned.as_field_value().as_bytes(), wire.as_bytes());
            assert_eq!(view.as_field_value().as_bytes(), wire.as_bytes());
        }
        for (wire, expected) in [
            (b"[v.abc]".as_slice(), DecodeErrorKind::InvalidSyntax),
            (b"[v1.]", DecodeErrorKind::InvalidSyntax),
            (b"[vG.abc]", DecodeErrorKind::InvalidSyntax),
            (b"[V1.a?]", DecodeErrorKind::InvalidSyntax),
            (b"[v1.%61]", DecodeErrorKind::InvalidSyntax),
            (b"[v1.a/b]", DecodeErrorKind::InvalidSyntax),
            (b"[v1.a@b]", DecodeErrorKind::InvalidSyntax),
            (b"[v1.a ]", DecodeErrorKind::InvalidSyntax),
            (b"[v1.a\xff]", DecodeErrorKind::InvalidSyntax),
            (b"[v1.a]suffix", DecodeErrorKind::InvalidSyntax),
            (b"[v1.a]:bad", DecodeErrorKind::InvalidNumber),
            (b"[v1.a]:bad@", DecodeErrorKind::InvalidSyntax),
        ] {
            assert_eq!(
                <Host as SingleValueField>::decode_view(FieldValueRef::new(wire))
                    .unwrap_err()
                    .kind(),
                expected,
                "{wire:?}"
            );
            assert_eq!(
                HostOwned::try_from(FieldValue::from_bytes(wire).unwrap()).unwrap_err().kind(),
                expected,
                "{wire:?}"
            );
        }
    }

    #[test]
    fn ipvfuture_dispatch_leaves_ipv6_values_unchanged() {
        for (wire, expected) in [
            ("[::]", Ipv6Addr::UNSPECIFIED),
            ("[::1]", Ipv6Addr::LOCALHOST),
            ("[DEAD:BEEF::1]", Ipv6Addr::new(0xdead, 0xbeef, 0, 0, 0, 0, 0, 1)),
            ("[::ffff:192.0.2.1]", Ipv4Addr::new(192, 0, 2, 1).to_ipv6_mapped()),
        ] {
            let owned = HostOwned::try_from(wire).unwrap();
            let view = <Host as SingleValueField>::decode_view(FieldValueRef::new(wire.as_bytes())).unwrap();
            assert_eq!(owned.kind(), HostKind::Ipv6(expected));
            assert_eq!(view.kind(), HostKind::Ipv6(expected));
            assert_eq!(view.port(), None);
            assert_eq!(view.network_port(), Ok(None));
            assert_eq!(owned.as_field_value().as_bytes(), wire.as_bytes());
            assert_eq!(view.as_field_value().as_bytes(), wire.as_bytes());
        }
    }

    #[test]
    fn ports_distinguish_absence_empty_zero_leading_zeros_and_overflow() {
        for (text, expected) in [
            (None, Ok(None)),
            (Some(""), Err(PortConversionErrorKind::Empty)),
            (Some("0"), Ok(Some(0))),
            (Some("00000"), Ok(Some(0))),
            (Some("000000000000000000000000000000000000000000443"), Ok(Some(443))),
            (Some("6553"), Ok(Some(6553))),
            (Some("6554"), Ok(Some(6554))),
            (Some("65530"), Ok(Some(65530))),
            (Some("65534"), Ok(Some(65534))),
            (Some("65535"), Ok(Some(65535))),
            (Some("000000000000000000000000000000000000000065535"), Ok(Some(65535))),
            (Some("65536"), Err(PortConversionErrorKind::Overflow)),
            (Some("65539"), Err(PortConversionErrorKind::Overflow)),
            (Some("65540"), Err(PortConversionErrorKind::Overflow)),
            (Some("655350"), Err(PortConversionErrorKind::Overflow)),
            (
                Some("000000000000000000000000000000000000000065536"),
                Err(PortConversionErrorKind::Overflow),
            ),
            (
                Some("655360000000000000000000000000000000000000000"),
                Err(PortConversionErrorKind::Overflow),
            ),
            (
                Some("999999999999999999999999999999999999999999999"),
                Err(PortConversionErrorKind::Overflow),
            ),
        ] {
            for host in ["example.com", "[::1]"] {
                let wire = text.map_or_else(|| host.to_owned(), |port| format!("{host}:{port}"));
                let owned = HostOwned::try_from(wire.as_str()).unwrap();
                let view = <Host as SingleValueField>::decode_view(FieldValueRef::new(wire.as_bytes())).unwrap();
                assert_eq!(owned.network_port().map_err(PortConversionError::kind), expected);
                assert_eq!(view.network_port().map_err(PortConversionError::kind), expected);
                assert_eq!(owned.port_view().map(HostPortView::as_str), text);
                assert_eq!(view.port_view().map(HostPortView::as_str), text);
                assert_eq!(view.port_view().map(|port| port.to_string()).as_deref(), text);
                assert_eq!(owned.port().unwrap(), text);
                assert_eq!(owned.as_field_value().as_bytes(), wire.as_bytes());
                assert_eq!(view.as_field_value().as_bytes(), wire.as_bytes());
                let typed = HostOwned::from_parts(view.kind(), text.map(|text| HostPortView::new(text).unwrap())).unwrap();
                assert_eq!(typed, owned);
            }
        }
    }

    #[test]
    fn port_conversion_errors_describe_empty_and_overflow_without_a_source() {
        for (text, kind, message) in [
            ("", PortConversionErrorKind::Empty, "the URI port is empty"),
            ("65536", PortConversionErrorKind::Overflow, "the URI port exceeds u16::MAX"),
        ] {
            let error = HostPortView::new(text).unwrap().to_u16().unwrap_err();
            assert_eq!(error.kind(), kind);
            assert_eq!(error.to_string(), message);
            assert!(error.source().is_none());
        }
    }

    #[test]
    fn port_overflow_never_masks_invalid_byte_errors() {
        for prefix in ["0", "65535", "65536", "999999999999999999999999999"] {
            for (suffix, name_error, literal_error) in [
                (b"x".as_slice(), DecodeErrorKind::InvalidNumber, DecodeErrorKind::InvalidNumber),
                (b"\t", DecodeErrorKind::InvalidNumber, DecodeErrorKind::InvalidNumber),
                (b"/", DecodeErrorKind::InvalidNumber, DecodeErrorKind::InvalidNumber),
                (b"x:", DecodeErrorKind::InvalidSyntax, DecodeErrorKind::InvalidNumber),
                (b":x", DecodeErrorKind::InvalidSyntax, DecodeErrorKind::InvalidNumber),
                (b"x@", DecodeErrorKind::InvalidSyntax, DecodeErrorKind::InvalidSyntax),
                (b"x:@", DecodeErrorKind::InvalidSyntax, DecodeErrorKind::InvalidSyntax),
                (b"x\x7f", DecodeErrorKind::InvalidSyntax, DecodeErrorKind::InvalidSyntax),
                (b"x\x00", DecodeErrorKind::InvalidSyntax, DecodeErrorKind::InvalidSyntax),
                (b"x\n", DecodeErrorKind::InvalidSyntax, DecodeErrorKind::InvalidSyntax),
                (b"x\xff", DecodeErrorKind::InvalidSyntax, DecodeErrorKind::InvalidSyntax),
            ] {
                for (host, expected) in [("example.com", name_error), ("[::1]", literal_error)] {
                    let mut wire = format!("{host}:{prefix}").into_bytes();
                    wire.extend_from_slice(suffix);
                    assert_eq!(
                        <Host as SingleValueField>::decode_view(FieldValueRef::new(&wire))
                            .unwrap_err()
                            .kind(),
                        expected,
                        "{wire:?}"
                    );
                    if let Ok(value) = FieldValue::from_bytes(&wire) {
                        assert_eq!(HostOwned::try_from(value).unwrap_err().kind(), expected, "{wire:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn relaxed_idna_is_retained_and_owned_views_share_normalized_storage() {
        let wire = "münich.example:000443";
        let bytes = FieldValue::try_from(wire).unwrap();
        assert_eq!(
            <Host as SingleValueField>::decode_view(bytes.as_field_value_ref())
                .unwrap_err()
                .kind(),
            DecodeErrorKind::InvalidSyntax
        );
        assert_eq!(
            <Host as SingleValueField>::decode_owned(bytes.clone()).unwrap_err().kind(),
            DecodeErrorKind::InvalidSyntax
        );
        let owned = <Host as SingleValueField>::decode_owned_with(bytes.clone(), DecodeMode::Relaxed).unwrap();
        let view = <Host as SingleValueField>::decode_view_with(bytes.as_field_value_ref(), DecodeMode::Relaxed).unwrap();
        let borrowed = owned.as_view();
        let HostKind::RegisteredName(name) = owned.kind() else {
            panic!("international name was not retained");
        };
        let HostKind::RegisteredName(borrowed_name) = borrowed.kind() else {
            panic!("borrowed international name was not retained");
        };
        assert_eq!(name.as_str(), "münich.example");
        assert_eq!(name.normalized(), "xn--mnich-kva.example");
        assert_eq!(name.normalized().as_ptr(), borrowed_name.normalized().as_ptr());
        assert_eq!(view.kind(), owned.kind());
        assert_eq!(view.network_port(), Ok(Some(443)));
        assert_eq!(HostOwned::from_parts(view.kind(), view.port_view()).unwrap(), owned);
        drop(borrowed);
        assert_eq!(owned.into_field_value().as_bytes(), wire.as_bytes());
        let mut sink = Values::new(&FieldName::Host, "placeholder");
        view.insert_into(&mut sink).unwrap();
        assert_eq!(sink.values[0].as_bytes(), wire.as_bytes());

        for wire in ["\u{200d}.example", "\u{00ad}", "münich@example", "[münich]", "münich:bad"] {
            let value = FieldValue::try_from(wire).unwrap();
            assert_eq!(
                <Host as SingleValueField>::decode_owned_with(value.clone(), DecodeMode::Relaxed)
                    .unwrap_err()
                    .kind(),
                DecodeErrorKind::InvalidSyntax
            );
            assert_eq!(
                <Host as SingleValueField>::decode_view_with(value.as_field_value_ref(), DecodeMode::Relaxed)
                    .unwrap_err()
                    .kind(),
                DecodeErrorKind::InvalidSyntax
            );
        }
    }

    #[test]
    fn typed_composition_preserves_idna_delimiter_validation() {
        let wire = "example\u{ff1a}80";
        let value = <Host as SingleValueField>::decode_owned_with(FieldValue::try_from(wire).unwrap(), DecodeMode::Relaxed).unwrap();
        let HostKind::RegisteredName(name) = value.kind() else {
            panic!("international registered name was not retained");
        };
        assert_eq!(name.normalized(), "example:80");
        assert_eq!(value.network_port(), Ok(None));
        assert_eq!(HostOwned::from_parts(value.kind(), None).unwrap(), value);
        assert_eq!(
            HostOwned::from_parts(value.kind(), Some(HostPortView::new("443").unwrap()))
                .unwrap_err()
                .kind(),
            DecodeErrorKind::InvalidSyntax
        );
        assert_eq!(
            <Host as SingleValueField>::decode_owned_with(FieldValue::try_from(format!("{wire}:443")).unwrap(), DecodeMode::Relaxed)
                .unwrap_err()
                .kind(),
            DecodeErrorKind::InvalidSyntax
        );
        for (wire, normalized, error) in [
            ("\u{ff3b}\u{ff1a}\u{ff1a}1\u{ff3d}", "[::1]", None),
            (
                "\u{ff3b}\u{ff1a}\u{ff1a}1\u{ff3d}\u{ff1a}80",
                "[::1]:80",
                Some(DecodeErrorKind::InvalidNumber),
            ),
        ] {
            let value = <Host as SingleValueField>::decode_owned_with(FieldValue::try_from(wire).unwrap(), DecodeMode::Relaxed).unwrap();
            let HostKind::RegisteredName(name) = value.kind() else {
                panic!("international registered name was not retained");
            };
            assert_eq!(name.normalized(), normalized);
            let typed = HostOwned::from_parts(value.kind(), Some(HostPortView::new("443").unwrap()));
            let decoded =
                <Host as SingleValueField>::decode_owned_with(FieldValue::try_from(format!("{wire}:443")).unwrap(), DecodeMode::Relaxed);
            if let Some(error) = error {
                assert_eq!(typed.unwrap_err().kind(), error);
                assert_eq!(decoded.unwrap_err().kind(), error);
            } else {
                assert_eq!(typed.unwrap(), decoded.unwrap());
            }
        }
    }

    #[test]
    fn exact_port_error_precedence_and_source_round_trips_are_preserved() {
        for (wire, kind) in [
            ("host:999999999999999x", DecodeErrorKind::InvalidNumber),
            ("host:999999999999999x:", DecodeErrorKind::InvalidSyntax),
            ("host:999999999999999@", DecodeErrorKind::InvalidSyntax),
            ("[::1]:999999999999999x", DecodeErrorKind::InvalidNumber),
            ("[::1]:999999999999999:", DecodeErrorKind::InvalidNumber),
            ("[::1]:999999999999999@", DecodeErrorKind::InvalidSyntax),
            ("[::1]suffix", DecodeErrorKind::InvalidSyntax),
        ] {
            let value = FieldValue::try_from(wire).unwrap();
            assert_eq!(HostOwned::try_from(value.clone()).unwrap_err().kind(), kind, "{wire}");
            assert_eq!(
                <Host as SingleValueField>::decode_view(value.as_field_value_ref())
                    .unwrap_err()
                    .kind(),
                kind,
                "{wire}"
            );
        }
        for (host, kind) in [
            ("host:bad", DecodeErrorKind::InvalidSyntax),
            ("host:", DecodeErrorKind::InvalidSyntax),
            ("[::1]:", DecodeErrorKind::InvalidNumber),
        ] {
            assert_eq!(HostOwned::with_port(host, 443).unwrap_err().kind(), kind);
        }

        let mut source = Values::new(&FieldName::Host, "[2001:0DB8::1]:000443");
        let owned = <Host as Field>::owned(&source).unwrap().unwrap();
        assert_eq!(<Host as Field>::view(&source).unwrap().unwrap().kind(), owned.kind());
        let mut borrowed_sink = Values::new(&FieldName::Host, "placeholder");
        <Host as Field>::view(&source)
            .unwrap()
            .unwrap()
            .insert_into(&mut borrowed_sink)
            .unwrap();
        assert_eq!(borrowed_sink.values, source.values);
        <Host as Field>::insert(&mut source, owned).unwrap();
        assert_eq!(source.values[0].as_bytes(), b"[2001:0DB8::1]:000443");
        source.values.push(source.values[0].clone());
        assert_eq!(
            <Host as Field>::view(&source).unwrap_err().kind(),
            DecodeErrorKind::UnexpectedMultipleValues
        );
        assert_eq!(
            <Host as Field>::owned(&source).unwrap_err().kind(),
            DecodeErrorKind::UnexpectedMultipleValues
        );
        source.values = vec![FieldValue::try_from("a".repeat(MAX_CUSTOM_FIELD_BYTES)).unwrap()];
        assert_eq!(
            <Host as Field>::view(&source).unwrap().unwrap().host().len(),
            MAX_CUSTOM_FIELD_BYTES
        );
        source.values = vec![FieldValue::try_from("a".repeat(MAX_CUSTOM_FIELD_BYTES + 1)).unwrap()];
        assert_eq!(
            <Host as Field>::view(&source).unwrap_err().kind(),
            DecodeErrorKind::SourceLimitExceeded
        );
        assert_eq!(
            <Host as Field>::owned(&source).unwrap_err().kind(),
            DecodeErrorKind::SourceLimitExceeded
        );
    }

    #[test]
    fn invalid_utf8_preserves_the_existing_entry_point_error_precedence() {
        let value = FieldValue::from_bytes(b"\xff").unwrap();
        assert_eq!(
            <Host as SingleValueField>::decode_view(value.as_field_value_ref())
                .unwrap_err()
                .kind(),
            DecodeErrorKind::InvalidSyntax
        );
        for mode in [DecodeMode::Strict, DecodeMode::Relaxed] {
            assert_eq!(
                <Host as SingleValueField>::decode_view_with(value.as_field_value_ref(), mode)
                    .unwrap_err()
                    .kind(),
                DecodeErrorKind::InvalidUtf8
            );
            assert_eq!(
                <Host as SingleValueField>::decode_owned_with(value.clone(), mode)
                    .unwrap_err()
                    .kind(),
                if mode == DecodeMode::Strict {
                    DecodeErrorKind::InvalidSyntax
                } else {
                    DecodeErrorKind::InvalidUtf8
                }
            );
        }
    }
}

#[cfg(feature = "headers-cors")]
mod origin {
    use std::net::{Ipv4Addr, Ipv6Addr};

    use http_headers::headers::{
        AccessControlAllowOrigin, AccessControlAllowOriginKind, AccessControlAllowOriginOwned, OriginDomainView, OriginHost, OriginScheme,
    };

    use super::*;

    #[test]
    fn wildcard_null_and_origins_have_independent_structured_and_wire_values() {
        for (wire, expected) in [
            (" \t*\t ", AccessControlAllowOriginKind::Wildcard),
            ("\tnull ", AccessControlAllowOriginKind::Null),
        ] {
            let source = Values::new(&FieldName::AccessControlAllowOrigin, wire);
            let owned = <AccessControlAllowOrigin as Field>::owned(&source).unwrap().unwrap();
            let view = <AccessControlAllowOrigin as Field>::view(&source).unwrap().unwrap();
            assert_eq!(view.kind(), expected);
            assert_eq!(owned.kind(), expected);
            assert_eq!(owned.as_view().kind(), expected);
            assert_eq!(view.origin(), None);
            assert_eq!(owned.origin().unwrap(), None);
            assert_eq!(view.as_field_value().as_bytes(), wire.as_bytes());
            assert_eq!(owned.as_field_value().as_bytes(), wire.as_bytes());
        }
        assert_ne!(
            AccessControlAllowOriginOwned::wildcard(),
            AccessControlAllowOriginOwned::try_from(" * ").unwrap()
        );
        assert_ne!(
            AccessControlAllowOriginOwned::null(),
            AccessControlAllowOriginOwned::try_from(" null ").unwrap()
        );
    }

    #[test]
    fn all_schemes_expose_explicit_and_effective_ports_and_omit_default_ports() {
        for (scheme, prefix, default) in [
            (OriginScheme::Ftp, "ftp", 21),
            (OriginScheme::Http, "http", 80),
            (OriginScheme::Https, "https", 443),
            (OriginScheme::Ws, "ws", 80),
            (OriginScheme::Wss, "wss", 443),
        ] {
            assert_eq!(scheme.to_string(), prefix);
            for port in [None, Some(0), Some(8443), Some(u16::MAX)] {
                let wire = port.map_or_else(
                    || format!("{prefix}://example.com."),
                    |port| format!("{prefix}://example.com.:{port}"),
                );
                let source = Values::new(&FieldName::AccessControlAllowOrigin, &wire);
                let owned = <AccessControlAllowOrigin as Field>::owned(&source).unwrap().unwrap();
                let view = <AccessControlAllowOrigin as Field>::view(&source).unwrap().unwrap();
                let AccessControlAllowOriginKind::Origin(tuple) = owned.kind() else {
                    panic!("serialized origin was not retained");
                };
                assert_eq!(view.kind(), owned.kind());
                assert_eq!(owned.as_view().kind(), view.kind());
                assert_eq!(tuple.as_str(), wire);
                assert_eq!(tuple.to_string(), wire);
                assert_eq!(tuple.scheme(), scheme);
                assert_eq!(tuple.port(), port);
                assert_eq!(tuple.effective_port(), port.unwrap_or(default));
                let OriginHost::Domain(domain) = tuple.host() else {
                    panic!("origin domain was not retained");
                };
                assert_eq!(domain.as_str(), "example.com.");
                assert_eq!(domain.to_string(), "example.com.");
                assert_eq!(
                    AccessControlAllowOriginOwned::from_parts(scheme, tuple.host(), port).unwrap(),
                    owned
                );
            }

            let domain = OriginHost::Domain(OriginDomainView::new("example.com").unwrap());
            let typed = AccessControlAllowOriginOwned::from_parts(scheme, domain, Some(default)).unwrap();
            assert_eq!(typed.as_str().unwrap(), format!("{prefix}://example.com"));
            let AccessControlAllowOriginKind::Origin(tuple) = typed.kind() else {
                panic!("typed origin was not retained");
            };
            assert_eq!(tuple.port(), None);
            assert_eq!(tuple.effective_port(), default);
            assert_eq!(
                AccessControlAllowOriginOwned::try_from(format!("{prefix}://example.com:{default}"))
                    .unwrap_err()
                    .kind(),
                DecodeErrorKind::InvalidSyntax
            );
        }
    }

    #[test]
    fn ip_addresses_are_retained_and_typed_ipv6_uses_url_canonicalization() {
        let ipv4 = Ipv4Addr::new(192, 0, 2, 128);
        let ipv6 = ipv4.to_ipv6_mapped();
        for (wire, host) in [
            ("https://192.0.2.128:8443", OriginHost::Ipv4(ipv4)),
            ("https://192.0.2.128.:8443", OriginHost::Ipv4(ipv4)),
            ("https://[::ffff:c000:280]:8443", OriginHost::Ipv6(ipv6)),
            ("https://[::1]:8443", OriginHost::Ipv6(Ipv6Addr::LOCALHOST)),
        ] {
            let source = Values::new(&FieldName::AccessControlAllowOrigin, wire);
            let owned = <AccessControlAllowOrigin as Field>::owned(&source).unwrap().unwrap();
            let view = <AccessControlAllowOrigin as Field>::view(&source).unwrap().unwrap();
            let AccessControlAllowOriginKind::Origin(tuple) = owned.kind() else {
                panic!("IP origin was not retained");
            };
            assert_eq!(tuple.host(), host);
            assert_eq!(tuple.port(), Some(8443));
            assert_eq!(tuple.effective_port(), 8443);
            assert_eq!(view.kind(), owned.kind());
            assert_eq!(owned.as_field_value().as_bytes(), wire.as_bytes());
        }
        for (port, expected, explicit) in [
            (None, "https://192.0.2.128", None),
            (Some(443), "https://192.0.2.128", None),
            (Some(8443), "https://192.0.2.128:8443", Some(8443)),
        ] {
            let typed = AccessControlAllowOriginOwned::from_parts(OriginScheme::Https, OriginHost::Ipv4(ipv4), port).unwrap();
            assert_eq!(typed.as_str().unwrap(), expected);
            let AccessControlAllowOriginKind::Origin(tuple) = typed.kind() else {
                panic!("typed IPv4 origin was not retained");
            };
            assert_eq!(tuple.host(), OriginHost::Ipv4(ipv4));
            assert_eq!(tuple.port(), explicit);
            assert_eq!(tuple.effective_port(), explicit.unwrap_or(443));
            assert_eq!(typed, AccessControlAllowOriginOwned::try_from(expected).unwrap());
        }
        let typed = AccessControlAllowOriginOwned::from_parts(OriginScheme::Https, OriginHost::Ipv6(ipv6), Some(443)).unwrap();
        assert_eq!(typed.as_str().unwrap(), "https://[::ffff:c000:280]");
        assert_eq!(typed, AccessControlAllowOriginOwned::try_from("https://[::ffff:c000:280]").unwrap());
        let ties = Ipv6Addr::new(0x2001, 0, 0, 1, 0, 0, 1, 1);
        assert_eq!(
            AccessControlAllowOriginOwned::from_parts(OriginScheme::Http, OriginHost::Ipv6(ties), None)
                .unwrap()
                .as_str()
                .unwrap(),
            "http://[2001::1:0:0:1:1]"
        );
    }

    #[test]
    fn canonical_ipv6_ascii_boundaries_preserve_components_and_serialization() {
        for (literal, expected) in [
            ("::", Ipv6Addr::UNSPECIFIED),
            ("::1", Ipv6Addr::LOCALHOST),
            ("1:2:3:4:5:6:7:8", Ipv6Addr::new(1, 2, 3, 4, 5, 6, 7, 8)),
            ("2001::1:0:0:1:1", Ipv6Addr::new(0x2001, 0, 0, 1, 0, 0, 1, 1)),
            ("ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff", Ipv6Addr::from([u16::MAX; 8])),
        ] {
            let serialized = format!("https://[{literal}]:8443");
            let wire = format!(" \t{serialized}\t ");
            let source = Values::new(&FieldName::AccessControlAllowOrigin, &wire);
            let owned = <AccessControlAllowOrigin as Field>::owned(&source).unwrap().unwrap();
            let view = <AccessControlAllowOrigin as Field>::view(&source).unwrap().unwrap();
            let AccessControlAllowOriginKind::Origin(tuple) = view.kind() else {
                panic!("canonical IPv6 origin was not retained");
            };
            assert_eq!(tuple.host(), OriginHost::Ipv6(expected));
            assert_eq!(tuple.scheme(), OriginScheme::Https);
            assert_eq!(tuple.port(), Some(8443));
            assert_eq!(tuple.effective_port(), 8443);
            assert_eq!(tuple.as_str(), serialized);
            assert_eq!(owned.kind(), view.kind());
            assert_eq!(owned.as_field_value().as_bytes(), wire.as_bytes());
            assert_eq!(view.as_field_value().as_bytes(), wire.as_bytes());
            let typed = AccessControlAllowOriginOwned::from_parts(OriginScheme::Https, OriginHost::Ipv6(expected), Some(8443)).unwrap();
            assert_eq!(typed.as_str().unwrap(), serialized);
            assert_eq!(typed.kind(), view.kind());
        }
    }

    #[test]
    fn ipv6_ascii_bounds_do_not_change_noncanonical_or_utf8_errors() {
        for literal in [
            "",
            "ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff0",
            "0ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff",
            "ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff:",
            "ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff::",
            "0:0:0:0:0:0:0:1",
            "ABCD::1",
            "2001:0db8::1",
            "::ffff:192.0.2.1",
            "::1%eth0",
            "::1\t",
            "\u{e9}",
        ] {
            let wire = format!("https://[{literal}]:8443");
            let source = Values::new(&FieldName::AccessControlAllowOrigin, &wire);
            for mode in [DecodeMode::Strict, DecodeMode::Relaxed] {
                assert_eq!(
                    <AccessControlAllowOrigin as Field>::view_with(&source, mode).unwrap_err().kind(),
                    DecodeErrorKind::InvalidSyntax,
                    "{wire}"
                );
                assert_eq!(
                    <AccessControlAllowOrigin as Field>::owned_with(&source, mode).unwrap_err().kind(),
                    DecodeErrorKind::InvalidSyntax,
                    "{wire}"
                );
            }
        }
        for prefix in ["", "ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff"] {
            let mut wire = format!("https://[{prefix}").into_bytes();
            wire.push(0xff);
            wire.extend_from_slice(b"]:8443");
            let source = Values {
                name: &FieldName::AccessControlAllowOrigin,
                values: vec![FieldValue::from_bytes(&wire).unwrap()],
            };
            for mode in [DecodeMode::Strict, DecodeMode::Relaxed] {
                assert_eq!(
                    <AccessControlAllowOrigin as Field>::view_with(&source, mode).unwrap_err().kind(),
                    DecodeErrorKind::InvalidUtf8
                );
                assert_eq!(
                    <AccessControlAllowOrigin as Field>::owned_with(&source, mode).unwrap_err().kind(),
                    DecodeErrorKind::InvalidUtf8
                );
            }
        }
    }

    #[test]
    fn broader_host_grammar_and_noncanonical_origin_forms_remain_rejected() {
        for wire in [
            "custom://example.com",
            "file://example.com",
            "HTTPS://example.com",
            "https://Example.com",
            "https://münich.example",
            "https://example%20host",
            "https://exa_mple",
            "https://[v1.alpha]",
            "https://127.1",
            "https://192.168.001.1",
            "https://[2001:0db8::1]",
            "https://[2001:DB8::1]",
            "https://[::ffff:192.0.2.128]",
            "https://example.com:443",
            "https://example.com:01",
            "https://example.com:65536",
            "https://example.com:",
            "https://user@example.com",
            "https://example.com/",
            "https://example.com?q",
            "https://example.com#fragment",
        ] {
            let source = Values::new(&FieldName::AccessControlAllowOrigin, wire);
            for mode in [DecodeMode::Strict, DecodeMode::Relaxed] {
                assert_eq!(
                    <AccessControlAllowOrigin as Field>::view_with(&source, mode).unwrap_err().kind(),
                    DecodeErrorKind::InvalidSyntax,
                    "{wire}"
                );
                assert_eq!(
                    <AccessControlAllowOrigin as Field>::owned_with(&source, mode).unwrap_err().kind(),
                    DecodeErrorKind::InvalidSyntax,
                    "{wire}"
                );
            }
        }
        for domain in [
            "Example.com",
            "münich.example",
            "127.0.0.1",
            "-invalid",
            "a..b",
            "name%",
            "",
            "a".repeat(64).as_str(),
        ] {
            assert_eq!(OriginDomainView::new(domain).unwrap_err().kind(), DecodeErrorKind::InvalidSyntax);
        }
    }

    #[test]
    fn typed_construction_preserves_http_authority_length_boundaries() {
        for (last_label, accepted) in [(59, true), (60, false), (61, false)] {
            let domain = format!(
                "{}.{}.{}.{}",
                "a".repeat(63),
                "b".repeat(63),
                "c".repeat(63),
                "d".repeat(last_label)
            );
            let host = OriginHost::Domain(OriginDomainView::new(&domain).unwrap());
            let typed = AccessControlAllowOriginOwned::from_parts(OriginScheme::Https, host, Some(0));
            let parsed = AccessControlAllowOriginOwned::try_from(format!("https://{domain}:0"));
            if accepted {
                assert_eq!(typed.unwrap(), parsed.unwrap());
            } else {
                assert_eq!(typed.unwrap_err().kind(), DecodeErrorKind::InvalidSyntax);
                assert_eq!(parsed.unwrap_err().kind(), DecodeErrorKind::InvalidSyntax);
            }
            let default_port = AccessControlAllowOriginOwned::from_parts(OriginScheme::Https, host, Some(443)).unwrap();
            assert_eq!(
                default_port,
                AccessControlAllowOriginOwned::try_from(format!("https://{domain}")).unwrap()
            );
            let ftp = AccessControlAllowOriginOwned::from_parts(OriginScheme::Ftp, host, Some(0)).unwrap();
            assert_eq!(ftp, AccessControlAllowOriginOwned::try_from(format!("ftp://{domain}:0")).unwrap());
        }
    }

    #[test]
    fn whitespace_encoding_duplicate_errors_and_source_limits_are_preserved() {
        let wire = " \thttps://[2001:db8::1]:65535\t ";
        let mut source = Values::new(&FieldName::AccessControlAllowOrigin, wire);
        let owned = <AccessControlAllowOrigin as Field>::owned(&source).unwrap().unwrap();
        assert_eq!(
            <AccessControlAllowOrigin as Field>::view(&source).unwrap().unwrap().kind(),
            owned.kind()
        );
        let mut borrowed_sink = Values::new(&FieldName::AccessControlAllowOrigin, "*");
        <AccessControlAllowOrigin as Field>::view(&source)
            .unwrap()
            .unwrap()
            .insert_into(&mut borrowed_sink)
            .unwrap();
        assert_eq!(borrowed_sink.values, source.values);
        <AccessControlAllowOrigin as Field>::insert(&mut source, owned).unwrap();
        assert_eq!(source.values[0].as_bytes(), wire.as_bytes());
        source.values.push(source.values[0].clone());
        assert_eq!(
            <AccessControlAllowOrigin as Field>::view(&source).unwrap_err().kind(),
            DecodeErrorKind::UnexpectedMultipleValues
        );
        assert_eq!(
            <AccessControlAllowOrigin as Field>::owned(&source).unwrap_err().kind(),
            DecodeErrorKind::UnexpectedMultipleValues
        );
        source.values = vec![FieldValue::try_from(format!("{}*", " ".repeat(MAX_CUSTOM_FIELD_BYTES - 1))).unwrap()];
        assert_eq!(
            <AccessControlAllowOrigin as Field>::view(&source).unwrap().unwrap().kind(),
            AccessControlAllowOriginKind::Wildcard
        );
        source.values = vec![FieldValue::try_from(format!("{}*", " ".repeat(MAX_CUSTOM_FIELD_BYTES))).unwrap()];
        assert_eq!(
            <AccessControlAllowOrigin as Field>::view(&source).unwrap_err().kind(),
            DecodeErrorKind::SourceLimitExceeded
        );
        assert_eq!(
            <AccessControlAllowOrigin as Field>::owned(&source).unwrap_err().kind(),
            DecodeErrorKind::SourceLimitExceeded
        );
        source.values = vec![FieldValue::from_bytes(b"\xff").unwrap()];
        assert_eq!(
            <AccessControlAllowOrigin as Field>::view(&source).unwrap_err().kind(),
            DecodeErrorKind::InvalidUtf8
        );
        assert_eq!(
            <AccessControlAllowOrigin as Field>::owned(&source).unwrap_err().kind(),
            DecodeErrorKind::InvalidUtf8
        );
    }
}
