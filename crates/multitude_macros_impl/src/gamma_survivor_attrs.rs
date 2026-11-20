// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use quote::ToTokens;
use syn::{Attribute, DeriveInput};

use super::{DefaultValue, RenameRule, parse_container, parse_field, parse_variant};

fn attributes(source: &str) -> Vec<Attribute> {
    syn::parse_str::<DeriveInput>(&format!("{source} struct Input;")).unwrap().attrs
}

#[test]
fn gamma_survivor_attribute_diagnostics() {
    let field_cases = [
        (r#"#[serde(rename = "a", rename = "b")]"#, "duplicate serde deserialize rename"),
        (
            r#"#[serde(rename(deserialize = "a", deserialize = "b"))]"#,
            "duplicate serde deserialize rename",
        ),
        (r#"#[serde(rename(other = "a"))]"#, "expected `serialize` or `deserialize`"),
        (r"#[multitude(via_serde, via_serde)]", "duplicate `via_serde`"),
        (r"#[multitude(default, default)]", "duplicate default"),
        (r"#[serde(default)] #[multitude(default)]", "duplicate default"),
        (r"#[serde(default, default)]", "duplicate default"),
        (
            r#"#[multitude(deserialize_with = "a", deserialize_with = "b")]"#,
            "duplicate `multitude(deserialize_with = ...)`",
        ),
        (
            r#"#[serde(deserialize_with = "a", deserialize_with = "b")]"#,
            "duplicate serde deserializer",
        ),
        (r#"#[serde(with = "a", with = "b")]"#, "duplicate serde deserializer"),
        (r#"#[serde(deserialize_with = "a", with = "b")]"#, "duplicate serde deserializer"),
        (r#"#[serde(with = "a", deserialize_with = "b")]"#, "duplicate serde deserializer"),
        (
            r#"#[multitude(unknown = "x")]"#,
            "unsupported `multitude` field attribute; expected `via_serde`, `deserialize_with`, `skip`, or `default`",
        ),
        (
            r#"#[serde(unknown = "x")]"#,
            "unsupported serde field attribute for `DeserializeIn`",
        ),
        (
            r"#[serde(borrow)]",
            "this serde field representation is not supported by `DeserializeIn`",
        ),
        (
            r"#[serde(flatten)]",
            "serde `flatten` cannot replay buffered Value fields because `&Value<A>` ties the Deserializer lifetime to its temporary borrow",
        ),
        (
            r#"#[serde(rename_all = "snake_case")]"#,
            "unsupported serde field attribute for `DeserializeIn`",
        ),
    ];
    for (source, message) in field_cases {
        assert_eq!(parse_field(&attributes(source)).err().unwrap().to_string(), message, "{source}");
    }

    let container_cases = [
        (r#"#[multitude(crate = "a", crate = "b")]"#, "duplicate `multitude(crate = ...)`"),
        (
            r#"#[multitude(other = "a")]"#,
            "unsupported `multitude` container attribute; expected `crate = \"...\"`",
        ),
        (r"#[serde(default, default)]", "duplicate serde container default"),
        (r"#[serde(transparent, transparent)]", "duplicate serde `transparent`"),
        (
            r#"#[serde(expecting = "a", expecting = "b")]"#,
            "duplicate serde container expectation",
        ),
        (
            r#"#[serde(bound = "T: Clone", bound = "T: Copy")]"#,
            "duplicate serde deserialize bound",
        ),
        (
            r#"#[serde(bound(deserialize = "T: Clone", deserialize = "T: Copy"))]"#,
            "duplicate serde deserialize bound",
        ),
        (r#"#[serde(bound(other = "T: Clone"))]"#, "expected `serialize` or `deserialize`"),
        (
            r#"#[serde(rename_all = "snake_case", rename_all = "kebab-case")]"#,
            "duplicate serde deserialize rename rule",
        ),
        (
            r#"#[serde(rename_all(deserialize = "snake_case", deserialize = "kebab-case"))]"#,
            "duplicate serde deserialize rename rule",
        ),
        (
            r#"#[serde(rename_all_fields = "snake_case", rename_all_fields = "kebab-case")]"#,
            "duplicate serde deserialize rename rule",
        ),
        (
            r#"#[serde(rename_all(other = "snake_case"))]"#,
            "expected `serialize` or `deserialize`",
        ),
        (r#"#[serde(rename_all = "unknown")]"#, "unknown serde rename rule"),
        (r#"#[serde(rename_all(deserialize = "unknown"))]"#, "unknown serde rename rule"),
        (r#"#[serde(rename_all(serialize = "unknown"))]"#, "unknown serde rename rule"),
        (
            r#"#[serde(remote = "Remote")]"#,
            "serde `remote` cannot implement the foreign `DeserializeIn` trait for a foreign type under Rust's orphan rules",
        ),
        (
            r#"#[serde(from = "Remote")]"#,
            "this serde container representation is not supported by `DeserializeIn`",
        ),
        (
            r#"#[serde(try_from = "Remote")]"#,
            "this serde container representation is not supported by `DeserializeIn`",
        ),
        (
            r#"#[serde(other = "x")]"#,
            "unsupported serde container attribute for `DeserializeIn`",
        ),
    ];
    for (source, message) in container_cases {
        assert_eq!(parse_container(&attributes(source)).err().unwrap().to_string(), message, "{source}");
    }
    for source in [r#"#[serde(tag = "kind")]"#, r#"#[serde(content = "data")]"#, "#[serde(untagged)]"] {
        assert_eq!(
            parse_container(&attributes(source)).err().unwrap().to_string(),
            "tagged and untagged serde enums cannot replay buffered Value payloads because `&Value<A>` ties the Deserializer lifetime to its temporary borrow",
            "{source}",
        );
    }
    let modes = [
        "#[multitude(via_serde)]",
        r#"#[serde(deserialize_with = "decode")]"#,
        r#"#[serde(with = "codec")]"#,
        r#"#[multitude(deserialize_with = "decode")]"#,
    ];
    for (index, left) in modes.iter().enumerate() {
        for right in &modes[index + 1..] {
            if index == 1 && *right == modes[2] {
                continue;
            }
            assert_eq!(
                parse_field(&attributes(&format!("{left} {right}"))).err().unwrap().to_string(),
                "`via_serde`, serde `deserialize_with`/`with`, and multitude `deserialize_with` are mutually exclusive",
            );
        }
        for skip in ["#[serde(skip)]", "#[serde(skip_deserializing)]", "#[multitude(skip)]"] {
            assert_eq!(
                parse_field(&attributes(&format!("{skip} {left}"))).err().unwrap().to_string(),
                "`skip` cannot be combined with a deserializer mode",
            );
        }
    }
}

#[test]
fn gamma_survivor_attribute_syntax_errors_are_returned() {
    for key in [
        "rename",
        "default",
        "alias",
        "deserialize_with",
        "with",
        "skip_serializing_if",
        "serialize_with",
        "getter",
    ] {
        for namespace in ["serde", "multitude"] {
            if namespace == "multitude" && !["default", "deserialize_with"].contains(&key) {
                continue;
            }
            let attrs = attributes(&format!("#[{namespace}({key} = 12)]"));
            assert_eq!(parse_field(&attrs).err().unwrap().to_string(), "expected string literal");
        }
    }
    for key in ["rename", "rename_all", "rename_all_fields", "bound", "expecting"] {
        assert_eq!(
            parse_container(&attributes(&format!("#[serde({key} = 12)]")))
                .err()
                .unwrap()
                .to_string(),
            "expected string literal",
        );
    }
    for key in ["rename", "rename_all", "rename_all_fields", "bound"] {
        for direction in ["serialize", "deserialize"] {
            let source = format!("#[serde({key}({direction} = 12))]");
            assert_eq!(
                parse_container(&attributes(&source)).err().unwrap().to_string(),
                "expected string literal"
            );
            let source = format!("#[serde({key}({direction}))]");
            assert_eq!(parse_container(&attributes(&source)).err().unwrap().to_string(), "expected `=`");
        }
    }
    for key in [
        "alias",
        "deserialize_with",
        "with",
        "skip_serializing_if",
        "serialize_with",
        "getter",
    ] {
        assert_eq!(
            parse_field(&attributes(&format!("#[serde({key})]"))).err().unwrap().to_string(),
            "expected `=`",
        );
    }
    for source in [
        r#"#[serde(default = "::")]"#,
        r#"#[multitude(default = "::")]"#,
        r#"#[serde(deserialize_with = "::")]"#,
        r#"#[multitude(deserialize_with = "::")]"#,
        r#"#[serde(with = "::")]"#,
    ] {
        assert_eq!(
            parse_field(&attributes(source)).err().unwrap().to_string(),
            "unexpected end of input, expected identifier"
        );
    }
    for source in [r#"#[multitude(crate = "::")]"#, r#"#[serde(default = "::")]"#] {
        assert_eq!(
            parse_container(&attributes(source)).err().unwrap().to_string(),
            "unexpected end of input, expected identifier"
        );
    }
    assert_eq!(
        parse_container(&attributes("#[multitude(crate = 12)]")).err().unwrap().to_string(),
        "expected string literal"
    );
    assert_eq!(
        parse_container(&attributes("#[multitude(crate)]")).err().unwrap().to_string(),
        "expected `=`"
    );
    assert!(parse_container(&attributes(r#"#[serde(bound = "T =")]"#)).is_err());
    assert!(parse_variant(&attributes(r#"#[serde(rename_all = "invalid")]"#)).is_err());
}

#[test]
fn gamma_survivor_attribute_values_and_rename_cases() {
    let attrs = attributes(
        r#"#[serde(rename(serialize = "out", deserialize = "wire"), alias = "old", alias = "older", default = "defaults::field")]
           #[multitude(deserialize_with = "custom::decode")]
           #[other(ignored)]
           #[serde(skip_serializing, skip_serializing_if = "omit", serialize_with = "write", getter = "get")]"#,
    );
    let field = parse_field(&attrs).unwrap();
    assert_eq!(field.rename.as_deref(), Some("wire"));
    assert_eq!(field.aliases, ["old", "older"]);
    assert!(!field.skip);
    assert!(!field.via_serde);
    assert!(field.serde_with.is_none());
    assert_eq!(field.multitude_with.unwrap().to_token_stream().to_string(), "custom :: decode");
    let Some(DefaultValue::Path(path)) = field.default else {
        panic!("path default was lost")
    };
    assert_eq!(path.to_token_stream().to_string(), "defaults :: field");
    assert!(
        parse_field(&attributes(r#"#[serde(rename(serialize = "out"))]"#))
            .unwrap()
            .rename
            .is_none()
    );
    assert!(matches!(
        parse_field(&attributes("#[multitude(default)]")).unwrap().default,
        Some(DefaultValue::Trait)
    ));
    assert!(parse_field(&attributes("#[multitude(via_serde)]")).unwrap().via_serde);
    assert_eq!(
        parse_field(&attributes(r#"#[serde(with = "::codec")]"#))
            .unwrap()
            .serde_with
            .unwrap()
            .to_token_stream()
            .to_string(),
        ":: codec :: deserialize",
    );

    let parsed = parse_container(&attributes(
        r#"#[multitude(crate = "::renamed")]
           #[serde(rename(deserialize = "Wire", serialize = "Out"), deny_unknown_fields,
             rename_all(deserialize = "kebab-case", serialize = "UPPERCASE"),
             rename_all_fields = "camelCase", default = "make",
             expecting = "a named record", bound(deserialize = "T: Clone, U: Copy", serialize = "V: Send"))]"#,
    ))
    .unwrap();
    assert_eq!(parsed.rename.as_deref(), Some("Wire"));
    assert!(parsed.deny_unknown_fields);
    assert!(!parsed.transparent);
    assert_eq!(parsed.multitude_crate.unwrap().to_token_stream().to_string(), ":: renamed");
    assert_eq!(parsed.expecting.as_deref(), Some("a named record"));
    assert_eq!(parsed.rename_all.unwrap().field("some_field"), "some-field");
    assert_eq!(parsed.rename_all_fields.unwrap().field("some_field"), "someField");
    let bounds: Vec<_> = parsed
        .deserialize_bounds
        .unwrap()
        .iter()
        .map(|bound| bound.to_token_stream().to_string())
        .collect();
    assert_eq!(bounds, ["T : Clone", "U : Copy"]);
    assert!(
        parse_container(&attributes(r#"#[serde(bound(serialize = "T: Clone"))]"#))
            .unwrap()
            .deserialize_bounds
            .is_none()
    );
    assert_eq!(
        parse_container(&attributes(r#"#[serde(bound = "")]"#))
            .unwrap()
            .deserialize_bounds
            .unwrap()
            .len(),
        0
    );
    assert!(parse_container(&attributes("#[serde(transparent)]")).unwrap().transparent);
    assert!(matches!(
        parse_container(&attributes("#[serde(default)]")).unwrap().default,
        Some(DefaultValue::Trait)
    ));

    let cases = [
        ("lowercase", "http_URL", "httpserver"),
        ("UPPERCASE", "HTTP_URL", "HTTPSERVER"),
        ("PascalCase", "HttpURL", "HTTPServer"),
        ("camelCase", "httpURL", "hTTPServer"),
        ("snake_case", "http_URL", "h_t_t_p_server"),
        ("SCREAMING_SNAKE_CASE", "HTTP_URL", "H_T_T_P_SERVER"),
        ("kebab-case", "http-URL", "h-t-t-p-server"),
        ("SCREAMING-KEBAB-CASE", "HTTP-URL", "H-T-T-P-SERVER"),
    ];
    for (rule, field, variant) in cases {
        let rule = RenameRule::parse(&syn::LitStr::new(rule, proc_macro2::Span::call_site())).unwrap();
        assert_eq!(rule.field("http_URL"), field);
        assert_eq!(rule.variant("HTTPServer"), variant);
        assert_eq!(rule.field(""), "");
        assert_eq!(rule.variant(""), "");
    }
    assert_eq!(RenameRule::Pascal.field("__two__words_"), "TwoWords");
    assert_eq!(RenameRule::Camel.field("_some_field"), "someField");
    assert_eq!(RenameRule::Snake.variant("ÉclairURL"), "Éclair_u_r_l".to_ascii_lowercase());
    assert_eq!(RenameRule::Camel.variant("Éclair"), "Éclair");
}
