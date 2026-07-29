// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use syn::punctuated::Punctuated;
use syn::{Attribute, LitStr, Path, WherePredicate};

#[derive(Clone)]
pub(crate) enum DefaultValue {
    Trait,
    Path(Path),
}

#[derive(Default, Clone)]
pub(crate) struct FieldAttrs {
    pub(crate) rename: Option<String>,
    pub(crate) rename_all: Option<RenameRule>,
    pub(crate) aliases: Vec<String>,
    pub(crate) default: Option<DefaultValue>,
    pub(crate) skip: bool,
    pub(crate) via_serde: bool,
    pub(crate) serde_with: Option<Path>,
    pub(crate) multitude_with: Option<Path>,
}

#[derive(Clone, Copy)]
pub(crate) enum RenameRule {
    Lower,
    Upper,
    Pascal,
    Camel,
    Snake,
    ScreamingSnake,
    Kebab,
    ScreamingKebab,
}

impl RenameRule {
    fn parse(value: &LitStr) -> syn::Result<Self> {
        match value.value().as_str() {
            "lowercase" => Ok(Self::Lower),
            "UPPERCASE" => Ok(Self::Upper),
            "PascalCase" => Ok(Self::Pascal),
            "camelCase" => Ok(Self::Camel),
            "snake_case" => Ok(Self::Snake),
            "SCREAMING_SNAKE_CASE" => Ok(Self::ScreamingSnake),
            "kebab-case" => Ok(Self::Kebab),
            "SCREAMING-KEBAB-CASE" => Ok(Self::ScreamingKebab),
            _ => Err(syn::Error::new_spanned(value, "unknown serde rename rule")),
        }
    }

    pub(crate) fn field(self, name: &str) -> String {
        match self {
            Self::Lower | Self::Snake => name.to_owned(),
            Self::Upper | Self::ScreamingSnake => name.to_ascii_uppercase(),
            Self::Pascal => name.split('_').map(capitalize).collect(),
            Self::Camel => {
                let pascal: String = name.split('_').map(capitalize).collect();
                lowercase_first(pascal)
            }
            Self::Kebab => name.replace('_', "-"),
            Self::ScreamingKebab => name.to_ascii_uppercase().replace('_', "-"),
        }
    }

    pub(crate) fn variant(self, name: &str) -> String {
        match self {
            Self::Lower => name.to_ascii_lowercase(),
            Self::Upper => name.to_ascii_uppercase(),
            Self::Pascal => name.to_owned(),
            Self::Camel => lowercase_first(name.to_owned()),
            Self::Snake => variant_words(name, "_").to_ascii_lowercase(),
            Self::ScreamingSnake => variant_words(name, "_").to_ascii_uppercase(),
            Self::Kebab => variant_words(name, "-").to_ascii_lowercase(),
            Self::ScreamingKebab => variant_words(name, "-").to_ascii_uppercase(),
        }
    }
}

fn capitalize(word: &str) -> String {
    let mut chars = word.chars();
    chars.next().map_or_else(String::new, |first| {
        std::iter::once(first.to_ascii_uppercase()).chain(chars).collect()
    })
}

fn lowercase_first(mut value: String) -> String {
    if let Some(first) = value.get_mut(0..1) {
        first.make_ascii_lowercase();
    }
    value
}

fn variant_words(value: &str, separator: &str) -> String {
    let mut output = String::new();
    for (index, ch) in value.char_indices() {
        if index > 0 && ch.is_uppercase() {
            output.push_str(separator);
        }
        output.push(ch);
    }
    output
}

#[derive(Default)]
pub(crate) struct ContainerAttrs {
    pub(crate) rename: Option<String>,
    pub(crate) deny_unknown_fields: bool,
    pub(crate) multitude_crate: Option<Path>,
    pub(crate) rename_all: Option<RenameRule>,
    pub(crate) rename_all_fields: Option<RenameRule>,
    pub(crate) default: Option<DefaultValue>,
    pub(crate) deserialize_bounds: Option<Vec<WherePredicate>>,
    pub(crate) expecting: Option<String>,
    pub(crate) transparent: bool,
}

fn parse_rename(meta: syn::meta::ParseNestedMeta<'_>, target: &mut Option<String>) -> syn::Result<()> {
    if meta.input.peek(syn::Token![=]) {
        if target.is_some() {
            return Err(meta.error("duplicate serde deserialize rename"));
        }
        *target = Some(meta.value()?.parse::<LitStr>()?.value());
        return Ok(());
    }
    meta.parse_nested_meta(|nested| {
        if nested.path.is_ident("deserialize") {
            if target.is_some() {
                return Err(nested.error("duplicate serde deserialize rename"));
            }
            *target = Some(nested.value()?.parse::<LitStr>()?.value());
        } else if nested.path.is_ident("serialize") {
            let _ = nested.value()?.parse::<LitStr>()?;
        } else {
            return Err(nested.error("expected `serialize` or `deserialize`"));
        }
        Ok(())
    })
}

fn parse_default(meta: syn::meta::ParseNestedMeta<'_>) -> syn::Result<DefaultValue> {
    if meta.input.peek(syn::Token![=]) {
        let value = meta.value()?.parse::<LitStr>()?;
        Ok(DefaultValue::Path(value.parse()?))
    } else {
        Ok(DefaultValue::Trait)
    }
}

fn parse_rule(meta: syn::meta::ParseNestedMeta<'_>, target: &mut Option<RenameRule>) -> syn::Result<()> {
    if meta.input.peek(syn::Token![=]) {
        if target.is_some() {
            return Err(meta.error("duplicate serde deserialize rename rule"));
        }
        *target = Some(RenameRule::parse(&meta.value()?.parse::<LitStr>()?)?);
        return Ok(());
    }
    meta.parse_nested_meta(|nested| {
        if nested.path.is_ident("deserialize") {
            if target.is_some() {
                return Err(nested.error("duplicate serde deserialize rename rule"));
            }
            *target = Some(RenameRule::parse(&nested.value()?.parse::<LitStr>()?)?);
        } else if nested.path.is_ident("serialize") {
            let _ = RenameRule::parse(&nested.value()?.parse::<LitStr>()?)?;
        } else {
            return Err(nested.error("expected `serialize` or `deserialize`"));
        }
        Ok(())
    })
}

fn set_custom_path(target: &mut Option<Path>, value: LitStr, message: &str) -> syn::Result<()> {
    if target.is_some() {
        return Err(syn::Error::new_spanned(value, message));
    }
    *target = Some(value.parse()?);
    Ok(())
}

fn parse_field_or_variant(attrs: &[Attribute], allow_variant_rename_all: bool) -> syn::Result<FieldAttrs> {
    let mut result = FieldAttrs::default();
    for attr in attrs {
        if attr.path().is_ident("multitude") {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("via_serde") {
                    if result.via_serde {
                        return Err(meta.error("duplicate `via_serde`"));
                    }
                    result.via_serde = true;
                } else if meta.path.is_ident("skip") {
                    result.skip = true;
                } else if meta.path.is_ident("default") {
                    if result.default.is_some() {
                        return Err(meta.error("duplicate default"));
                    }
                    result.default = Some(parse_default(meta)?);
                } else if meta.path.is_ident("deserialize_with") {
                    let value = meta.value()?.parse::<LitStr>()?;
                    set_custom_path(&mut result.multitude_with, value, "duplicate `multitude(deserialize_with = ...)`")?;
                } else {
                    return Err(meta
                        .error("unsupported `multitude` field attribute; expected `via_serde`, `deserialize_with`, `skip`, or `default`"));
                }
                Ok(())
            })?;
        } else if attr.path().is_ident("serde") {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("rename") {
                    parse_rename(meta, &mut result.rename)?;
                } else if allow_variant_rename_all && meta.path.is_ident("rename_all") {
                    parse_rule(meta, &mut result.rename_all)?;
                } else if meta.path.is_ident("alias") {
                    result.aliases.push(meta.value()?.parse::<LitStr>()?.value());
                } else if meta.path.is_ident("default") {
                    if result.default.is_some() {
                        return Err(meta.error("duplicate default"));
                    }
                    result.default = Some(parse_default(meta)?);
                } else if meta.path.is_ident("skip") || meta.path.is_ident("skip_deserializing") {
                    result.skip = true;
                } else if meta.path.is_ident("deserialize_with") {
                    let value = meta.value()?.parse::<LitStr>()?;
                    set_custom_path(&mut result.serde_with, value, "duplicate serde deserializer")?;
                } else if meta.path.is_ident("with") {
                    let value = meta.value()?.parse::<LitStr>()?;
                    let mut path: Path = value.parse()?;
                    path.segments.push(syn::parse_quote!(deserialize));
                    if result.serde_with.replace(path).is_some() {
                        return Err(meta.error("duplicate serde deserializer"));
                    }
                } else if meta.path.is_ident("flatten") {
                    return Err(meta.error(
                        "serde `flatten` cannot replay buffered Value fields because `&Value<A>` ties the Deserializer lifetime to its temporary borrow",
                    ));
                } else if meta.path.is_ident("borrow") {
                    return Err(meta.error("this serde field representation is not supported by `DeserializeIn`"));
                } else if meta.path.is_ident("skip_serializing") {
                    // Serialization-only configuration has no effect here.
                } else if meta.path.is_ident("skip_serializing_if") || meta.path.is_ident("serialize_with") || meta.path.is_ident("getter")
                {
                    let _ = meta.value()?.parse::<LitStr>()?;
                } else {
                    return Err(meta.error("unsupported serde field attribute for `DeserializeIn`"));
                }
                Ok(())
            })?;
        }
    }
    let custom_count =
        usize::from(result.via_serde) + usize::from(result.serde_with.is_some()) + usize::from(result.multitude_with.is_some());
    if custom_count > 1 {
        return Err(syn::Error::new_spanned(
            attrs.first().expect("attributes exist when deserializer modes conflict"),
            "`via_serde`, serde `deserialize_with`/`with`, and multitude `deserialize_with` are mutually exclusive",
        ));
    }
    if result.skip && (result.serde_with.is_some() || result.multitude_with.is_some() || result.via_serde) {
        return Err(syn::Error::new_spanned(
            attrs.first().expect("attributes exist when skip conflicts"),
            "`skip` cannot be combined with a deserializer mode",
        ));
    }
    Ok(result)
}

pub(crate) fn parse_field(attrs: &[Attribute]) -> syn::Result<FieldAttrs> {
    parse_field_or_variant(attrs, false)
}

pub(crate) fn parse_variant(attrs: &[Attribute]) -> syn::Result<FieldAttrs> {
    parse_field_or_variant(attrs, true)
}

pub(crate) fn parse_container(attrs: &[Attribute]) -> syn::Result<ContainerAttrs> {
    let mut result = ContainerAttrs::default();
    for attr in attrs {
        if attr.path().is_ident("multitude") {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("crate") {
                    if result.multitude_crate.is_some() {
                        return Err(meta.error("duplicate `multitude(crate = ...)`"));
                    }
                    let value = meta.value()?.parse::<LitStr>()?;
                    result.multitude_crate = Some(value.parse()?);
                    Ok(())
                } else {
                    Err(meta.error("unsupported `multitude` container attribute; expected `crate = \"...\"`"))
                }
            })?;
        } else if attr.path().is_ident("serde") {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("rename") {
                    parse_rename(meta, &mut result.rename)?;
                } else if meta.path.is_ident("rename_all") {
                    parse_rule(meta, &mut result.rename_all)?;
                } else if meta.path.is_ident("rename_all_fields") {
                    parse_rule(meta, &mut result.rename_all_fields)?;
                } else if meta.path.is_ident("deny_unknown_fields") {
                    result.deny_unknown_fields = true;
                } else if meta.path.is_ident("default") {
                    if result.default.is_some() {
                        return Err(meta.error("duplicate serde container default"));
                    }
                    result.default = Some(parse_default(meta)?);
                } else if meta.path.is_ident("transparent") {
                    if result.transparent {
                        return Err(meta.error("duplicate serde `transparent`"));
                    }
                    result.transparent = true;
                } else if meta.path.is_ident("tag") || meta.path.is_ident("content") || meta.path.is_ident("untagged") {
                    return Err(meta.error(
                        "tagged and untagged serde enums cannot replay buffered Value payloads because `&Value<A>` ties the Deserializer lifetime to its temporary borrow",
                    ));
                } else if meta.path.is_ident("remote") {
                    return Err(meta.error(
                        "serde `remote` cannot implement the foreign `DeserializeIn` trait for a foreign type under Rust's orphan rules",
                    ));
                } else if meta.path.is_ident("from")
                    || meta.path.is_ident("try_from")
                {
                    return Err(meta.error("this serde container representation is not supported by `DeserializeIn`"));
                } else if meta.path.is_ident("bound") {
                    let deserialize = if meta.input.peek(syn::Token![=]) {
                        Some(meta.value()?.parse::<LitStr>()?)
                    } else {
                        let mut deserialize = None;
                        meta.parse_nested_meta(|nested| {
                            if nested.path.is_ident("deserialize") {
                                if deserialize.is_some() {
                                    return Err(nested.error("duplicate serde deserialize bound"));
                                }
                                deserialize = Some(nested.value()?.parse::<LitStr>()?);
                            } else if nested.path.is_ident("serialize") {
                                let _ = nested.value()?.parse::<LitStr>()?;
                            } else {
                                return Err(nested.error("expected `serialize` or `deserialize`"));
                            }
                            Ok(())
                        })?;
                        deserialize
                    };
                    if let Some(value) = deserialize {
                        if result.deserialize_bounds.is_some() {
                            return Err(meta.error("duplicate serde deserialize bound"));
                        }
                        result.deserialize_bounds = Some(
                            value
                                .parse_with(Punctuated::<WherePredicate, syn::Token![,]>::parse_terminated)?
                                .into_iter()
                                .collect(),
                        );
                    }
                } else if meta.path.is_ident("expecting") {
                    if result.expecting.is_some() {
                        return Err(meta.error("duplicate serde container expectation"));
                    }
                    result.expecting = Some(meta.value()?.parse::<LitStr>()?.value());
                } else {
                    return Err(meta.error("unsupported serde container attribute for `DeserializeIn`"));
                }
                Ok(())
            })?;
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use quote::ToTokens;
    use syn::parse_quote;

    use super::*;

    #[test]
    fn parses_custom_crate_path() {
        let attrs: Vec<Attribute> = vec![parse_quote!(#[multitude(crate = "renamed_multitude")])];
        let parsed = parse_container(&attrs).unwrap();
        assert_eq!(parsed.multitude_crate.unwrap().to_token_stream().to_string(), "renamed_multitude");
    }

    #[test]
    fn rejects_duplicate_custom_crate_paths() {
        let attrs: Vec<Attribute> = vec![parse_quote!(#[multitude(crate = "one")]), parse_quote!(#[multitude(crate = "two")])];
        assert!(parse_container(&attrs).is_err());
    }

    #[test]
    fn ignores_serialization_only_field_attributes() {
        let attrs: Vec<Attribute> = vec![parse_quote!(
            #[serde(
                skip_serializing,
                skip_serializing_if = "Option::is_none",
                serialize_with = "serialize_value",
                getter = "Value::get"
            )]
        )];
        parse_field(&attrs).unwrap();
    }

    #[test]
    fn parses_deserialize_specific_rename_and_bound() {
        let attrs: Vec<Attribute> = vec![parse_quote!(
            #[serde(
                rename_all(deserialize = "kebab-case", serialize = "camelCase"),
                bound(deserialize = "T: Trait<'de>", serialize = "T: Other")
            )]
        )];
        let parsed = parse_container(&attrs).unwrap();
        assert_eq!(parsed.rename_all.unwrap().field("some_field"), "some-field");
        assert_eq!(parsed.deserialize_bounds.unwrap().len(), 1);
    }

    #[test]
    fn variant_rename_all_is_parsed_only_for_variants() {
        let attrs: Vec<Attribute> = vec![parse_quote!(
            #[serde(rename_all(deserialize = "camelCase", serialize = "snake_case"))]
        )];
        let parsed = parse_variant(&attrs).unwrap();
        assert_eq!(parsed.rename_all.unwrap().field("some_field"), "someField");
        assert!(parse_field(&attrs).is_err());
    }

    #[test]
    fn custom_deserializer_modes_conflict() {
        let attrs: Vec<Attribute> = vec![
            parse_quote!(#[multitude(via_serde)]),
            parse_quote!(#[serde(deserialize_with = "deserialize_value")]),
        ];
        assert!(parse_field(&attrs).is_err());
    }

    #[test]
    fn parses_transparent_and_rejects_buffered_representations() {
        let transparent: Vec<Attribute> = vec![parse_quote!(#[serde(transparent)])];
        assert!(parse_container(&transparent).unwrap().transparent);

        for attribute in [
            parse_quote!(#[serde(untagged)]),
            parse_quote!(#[serde(tag = "kind")]),
            parse_quote!(#[serde(tag = "kind", content = "value")]),
        ] {
            assert!(
                parse_container(&[attribute])
                    .err()
                    .expect("representation must be rejected")
                    .to_string()
                    .contains("temporary borrow")
            );
        }
        let flatten: Vec<Attribute> = vec![parse_quote!(#[serde(flatten)])];
        assert!(
            parse_field(&flatten)
                .err()
                .expect("flatten must be rejected")
                .to_string()
                .contains("temporary borrow")
        );
    }

    #[test]
    fn rename_rules_cover_every_field_variant_and_error() {
        let cases = [
            (RenameRule::Lower, "http_URL", "http_URL"),
            (RenameRule::Upper, "http_URL", "HTTP_URL"),
            (RenameRule::Pascal, "http_URL", "HttpURL"),
            (RenameRule::Camel, "http_URL", "httpURL"),
            (RenameRule::Snake, "http_URL", "http_URL"),
            (RenameRule::ScreamingSnake, "http_URL", "HTTP_URL"),
            (RenameRule::Kebab, "http_URL", "http-URL"),
            (RenameRule::ScreamingKebab, "http_URL", "HTTP-URL"),
        ];
        for (rule, input, expected) in cases {
            assert_eq!(rule.field(input), expected);
        }
        assert_eq!(RenameRule::Pascal.field("\u{e9}_field"), "\u{e9}Field");
        assert_eq!(RenameRule::Pascal.field(""), "");

        let variants = [
            (RenameRule::Lower, "HTTPServer", "httpserver"),
            (RenameRule::Upper, "HTTPServer", "HTTPSERVER"),
            (RenameRule::Pascal, "HTTPServer", "HTTPServer"),
            (RenameRule::Camel, "HTTPServer", "hTTPServer"),
            (RenameRule::Snake, "HTTPServer", "h_t_t_p_server"),
            (RenameRule::ScreamingSnake, "HTTPServer", "H_T_T_P_SERVER"),
            (RenameRule::Kebab, "HTTPServer", "h-t-t-p-server"),
            (RenameRule::ScreamingKebab, "HTTPServer", "H-T-T-P-SERVER"),
        ];
        for (rule, input, expected) in variants {
            assert_eq!(rule.variant(input), expected);
        }
        assert_eq!(RenameRule::Camel.variant(""), "");

        let invalid = LitStr::new("invalid", proc_macro2::Span::call_site());
        assert!(RenameRule::parse(&invalid).is_err());
    }

    #[test]
    fn field_attributes_cover_supported_and_conflicting_forms() {
        let attrs: Vec<Attribute> = vec![
            parse_quote!(#[multitude(default = "make_default", deserialize_with = "arena_decode")]),
            parse_quote!(#[serde(rename(serialize = "ignored", deserialize = "wire"), alias = "old")]),
        ];
        let parsed = parse_field(&attrs).unwrap();
        assert_eq!(parsed.rename.as_deref(), Some("wire"));
        assert_eq!(parsed.aliases, ["old"]);
        assert!(matches!(parsed.default, Some(DefaultValue::Path(_))));
        assert!(parsed.multitude_with.is_some());

        let with: Vec<Attribute> = vec![parse_quote!(#[serde(with = "codec")])];
        assert_eq!(
            parse_field(&with).unwrap().serde_with.unwrap().to_token_stream().to_string(),
            "codec :: deserialize"
        );

        let skipped: Vec<Attribute> = vec![parse_quote!(#[multitude(skip)])];
        let skipped = parse_field(&skipped).unwrap();
        assert!(skipped.skip);
        assert!(skipped.default.is_none());

        let skipped_default: Vec<Attribute> = vec![parse_quote!(#[serde(skip, default)])];
        assert!(matches!(parse_field(&skipped_default).unwrap().default, Some(DefaultValue::Trait)));
        let duplicate_default: Vec<Attribute> = vec![parse_quote!(#[serde(default, default = "make")])];
        assert!(parse_field(&duplicate_default).is_err());
        let cross_namespace_default: Vec<Attribute> = vec![parse_quote!(#[serde(default)]), parse_quote!(#[multitude(default = "make")])];
        assert!(parse_field(&cross_namespace_default).is_err());

        let skip_deserializing: Vec<Attribute> = vec![parse_quote!(#[serde(skip_deserializing)])];
        assert!(parse_field(&skip_deserializing).unwrap().skip);

        let duplicate_rename: Vec<Attribute> = vec![parse_quote!(#[serde(rename = "one", rename = "two")])];
        assert!(parse_field(&duplicate_rename).is_err());
        let duplicate_nested: Vec<Attribute> = vec![parse_quote!(#[serde(rename(deserialize = "one", deserialize = "two"))])];
        assert!(parse_field(&duplicate_nested).is_err());
        let invalid_nested: Vec<Attribute> = vec![parse_quote!(#[serde(rename(other = "bad"))])];
        assert!(parse_field(&invalid_nested).is_err());
        let duplicate_multitude: Vec<Attribute> = vec![parse_quote!(
            #[multitude(deserialize_with = "one", deserialize_with = "two")]
        )];
        assert!(parse_field(&duplicate_multitude).is_err());
        let duplicate_serde: Vec<Attribute> = vec![parse_quote!(#[serde(deserialize_with = "one", deserialize_with = "two")])];
        assert!(parse_field(&duplicate_serde).is_err());
        let duplicate_with: Vec<Attribute> = vec![parse_quote!(#[serde(with = "one", with = "two")])];
        assert!(parse_field(&duplicate_with).is_err());
        let duplicate_via: Vec<Attribute> = vec![parse_quote!(#[multitude(via_serde, via_serde)])];
        assert!(parse_field(&duplicate_via).is_err());
        let unsupported_multitude: Vec<Attribute> = vec![parse_quote!(#[multitude(other)])];
        assert!(parse_field(&unsupported_multitude).is_err());
        let unsupported_serde: Vec<Attribute> = vec![parse_quote!(#[serde(other)])];
        assert!(parse_field(&unsupported_serde).is_err());
        let borrowed: Vec<Attribute> = vec![parse_quote!(#[serde(borrow)])];
        assert!(parse_field(&borrowed).is_err());
        for skip_conflict in [
            vec![parse_quote!(#[serde(skip, with = "codec")])],
            vec![parse_quote!(#[multitude(skip, deserialize_with = "decode")])],
            vec![parse_quote!(#[multitude(skip, via_serde)])],
        ] {
            assert!(parse_field(&skip_conflict).is_err());
        }
        let unrelated: Vec<Attribute> = vec![parse_quote!(#[doc = "ignored"])];
        parse_field(&unrelated).unwrap();
    }

    #[test]
    fn container_attributes_cover_supported_and_rejected_forms() {
        let attrs: Vec<Attribute> = vec![parse_quote!(
            #[serde(
                rename(deserialize = "Wire", serialize = "Ignored"),
                rename_all_fields = "SCREAMING_SNAKE_CASE",
                deny_unknown_fields,
                default,
                bound = "T: Clone",
                expecting = "a value"
            )]
        )];
        let parsed = parse_container(&attrs).unwrap();
        assert_eq!(parsed.rename.as_deref(), Some("Wire"));
        assert!(parsed.deny_unknown_fields);
        assert!(matches!(parsed.default.as_ref(), Some(DefaultValue::Trait)));
        assert!(parsed.deserialize_bounds.is_some());
        assert_eq!(parsed.expecting.as_deref(), Some("a value"));
        assert_eq!(parsed.rename_all_fields.unwrap().field("some_field"), "SOME_FIELD");

        let default_path: Vec<Attribute> = vec![parse_quote!(#[serde(default = "make_default")])];
        let parsed = parse_container(&default_path).unwrap();
        let path_string = |default| match default {
            DefaultValue::Path(path) => Some(path.to_token_stream().to_string()),
            DefaultValue::Trait => None,
        };
        assert_eq!(parsed.default.and_then(path_string).as_deref(), Some("make_default"));
        assert_eq!(Some(DefaultValue::Trait).and_then(path_string), None);

        let nested_bound: Vec<Attribute> = vec![parse_quote!(#[serde(bound(serialize = "T: Copy"))])];
        assert!(parse_container(&nested_bound).unwrap().deserialize_bounds.is_none());

        for unsupported_conversion in [
            vec![parse_quote!(#[serde(from = "Wire")])],
            vec![parse_quote!(#[serde(try_from = "Wire")])],
        ] {
            assert!(
                parse_container(&unsupported_conversion)
                    .err()
                    .expect("conversion representation must be rejected")
                    .to_string()
                    .contains("container representation")
            );
        }

        let nested_rule: Vec<Attribute> = vec![parse_quote!(#[serde(rename_all(deserialize = "snake_case", serialize = "camelCase"))])];
        assert_eq!(
            parse_container(&nested_rule).unwrap().rename_all.unwrap().field("some_field"),
            "some_field"
        );

        for attributes in [
            vec![parse_quote!(#[serde(default, default)])],
            vec![parse_quote!(#[serde(expecting = "one", expecting = "two")])],
            vec![parse_quote!(#[serde(transparent, transparent)])],
            vec![parse_quote!(#[serde(rename_all = "lowercase", rename_all = "UPPERCASE")])],
            vec![parse_quote!(#[serde(rename_all(deserialize = "lowercase", deserialize = "UPPERCASE"))])],
            vec![parse_quote!(#[serde(rename_all(other = "lowercase"))])],
            vec![parse_quote!(#[serde(rename_all = "invalid")])],
            vec![parse_quote!(#[serde(remote = "Remote")])],
            vec![parse_quote!(#[serde(from = "Source")])],
            vec![parse_quote!(#[serde(try_from = "Source")])],
            vec![parse_quote!(#[serde(other)])],
            vec![parse_quote!(#[multitude(other)])],
        ] {
            assert!(parse_container(&attributes).is_err());
        }

        let duplicate_bound: Vec<Attribute> = vec![parse_quote!(#[serde(bound = "T: Clone", bound(deserialize = "T: Copy"))])];
        assert!(parse_container(&duplicate_bound).is_err());
        let duplicate_nested_bound: Vec<Attribute> = vec![parse_quote!(#[serde(bound(deserialize = "T: Clone", deserialize = "T: Copy"))])];
        assert!(parse_container(&duplicate_nested_bound).is_err());
        let invalid_bound: Vec<Attribute> = vec![parse_quote!(#[serde(bound(other = "T: Clone"))])];
        assert!(parse_container(&invalid_bound).is_err());
        let unrelated: Vec<Attribute> = vec![parse_quote!(#[doc = "ignored"])];
        parse_container(&unrelated).unwrap();
    }
}
