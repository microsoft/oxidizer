// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Attribute parsing for the `DeserializeIn` and `SerializeIn` derives.
//!
//! Two attribute namespaces are recognized:
//!
//! * `#[internity(...)]` — internity-specific configuration (`crate = "..."`,
//!   `via_serde`).
//! * `#[serde(...)]` — the subset of Serde's field-schema attributes that affect
//!   the wire format, so the interner-aware derives stay aligned with Serde's
//!   own `Serialize` and `Deserialize` derives.

use syn::spanned::Spanned as _;
use syn::{Attribute, LitStr, Path};

/// A Serde `rename_all` / `rename_all_fields` casing rule.
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

    /// Applies the rule to a `snake_case` Rust field identifier.
    pub(crate) fn apply(self, name: &str) -> String {
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
}

fn capitalize(word: &str) -> String {
    let mut chars = word.chars();
    chars.next().map_or_else(String::new, |first| {
        core::iter::once(first.to_ascii_uppercase()).chain(chars).collect()
    })
}

fn lowercase_first(mut value: String) -> String {
    if let Some(first) = value.get_mut(0..1) {
        first.make_ascii_lowercase();
    }
    value
}

/// A resolved `default` source for a field or container.
#[derive(Clone)]
pub(crate) enum DefaultValue {
    /// `#[serde(default)]` — `Default::default()`.
    Trait,
    /// `#[serde(default = "path")]` — call the named function.
    Path(Path),
}

/// Field-level configuration parsed from `#[internity(...)]` and `#[serde(...)]`.
#[derive(Default)]
#[expect(clippy::struct_excessive_bools, reason = "these are independent serde flags, not a state machine")]
pub(crate) struct FieldAttrs {
    /// `#[serde(rename = "...")]` (deserialize name).
    pub(crate) rename: Option<String>,
    /// `#[serde(rename = "...")]` (serialize name).
    pub(crate) serialize_rename: Option<String>,
    /// `#[serde(alias = "...")]` — additional accepted keys.
    pub(crate) aliases: Vec<String>,
    /// `#[serde(default)]` / `#[serde(default = "path")]`.
    pub(crate) default: Option<DefaultValue>,
    /// `#[serde(skip)]` / `#[serde(skip_deserializing)]`.
    pub(crate) skip: bool,
    /// `#[serde(skip)]` / `#[serde(skip_serializing)]`.
    pub(crate) skip_serializing: bool,
    /// `#[internity(via_serde)]` — deserialize via ordinary [`serde::Deserialize`].
    pub(crate) via_serde: bool,
    /// `#[serde(with = "...")]` / `#[serde(deserialize_with = "...")]` — the
    /// resolved path to a `deserialize` function.
    pub(crate) with: Option<Path>,
    /// `#[serde(with = "...")]` / `#[serde(serialize_with = "...")]` — the
    /// resolved path to a `serialize` function.
    pub(crate) serialize_with: Option<Path>,
    /// `#[serde(skip_serializing_if = "...")]` — present but unsupported by
    /// `SerializeIn` (rejected during serialize expansion, ignored by
    /// `DeserializeIn`).
    pub(crate) skip_serializing_if: bool,
}

/// Container-level configuration parsed from `#[internity(...)]` and `#[serde(...)]`.
#[derive(Default)]
pub(crate) struct ContainerAttrs {
    /// `#[internity(crate = "...")]` — path to the `internity` crate.
    pub(crate) internity_crate: Option<Path>,
    /// `#[serde(rename = "...")]` (deserialize name).
    pub(crate) rename: Option<String>,
    /// `#[serde(rename = "...")]` (serialize name).
    pub(crate) serialize_rename: Option<String>,
    /// `#[serde(rename_all = "...")]`.
    pub(crate) rename_all: Option<RenameRule>,
    /// `#[serde(rename_all = "...")]` for serialization.
    pub(crate) serialize_rename_all: Option<RenameRule>,
    /// `#[serde(deny_unknown_fields)]`.
    pub(crate) deny_unknown_fields: bool,
    /// `#[serde(default)]` / `#[serde(default = "path")]`.
    pub(crate) default: Option<DefaultValue>,
    /// `#[serde(transparent)]`.
    pub(crate) transparent: bool,
    /// `#[serde(expecting = "...")]`.
    pub(crate) expecting: Option<String>,
    /// `#[serde(from = "...")]` — deserialize-only conversion. Recorded here so
    /// each direction's expander can reject it only when it affects that
    /// direction (see [`crate::expand_deserialize`] / [`crate::expand_serialize`]).
    pub(crate) from: Option<proc_macro2::Span>,
    /// `#[serde(try_from = "...")]` — deserialize-only conversion.
    pub(crate) try_from: Option<proc_macro2::Span>,
    /// `#[serde(into = "...")]` — serialize-only conversion.
    pub(crate) into: Option<proc_macro2::Span>,
}

/// Parses a `rename` / container-level rename target, honoring the
/// `rename(deserialize = "...", serialize = "...")` split.
fn parse_rename(
    meta: &syn::meta::ParseNestedMeta<'_>,
    deserialize_target: &mut Option<String>,
    serialize_target: &mut Option<String>,
) -> syn::Result<()> {
    if meta.input.peek(syn::Token![=]) {
        if deserialize_target.is_some() || serialize_target.is_some() {
            return Err(meta.error("duplicate serde rename"));
        }
        let value = meta.value()?.parse::<LitStr>()?.value();
        *deserialize_target = Some(value.clone());
        *serialize_target = Some(value);
        return Ok(());
    }
    meta.parse_nested_meta(|nested| {
        if nested.path.is_ident("deserialize") {
            if deserialize_target.is_some() {
                return Err(nested.error("duplicate serde deserialize rename"));
            }
            *deserialize_target = Some(nested.value()?.parse::<LitStr>()?.value());
        } else if nested.path.is_ident("serialize") {
            if serialize_target.is_some() {
                return Err(nested.error("duplicate serde serialize rename"));
            }
            *serialize_target = Some(nested.value()?.parse::<LitStr>()?.value());
        } else {
            return Err(nested.error("expected `serialize` or `deserialize`"));
        }
        Ok(())
    })
}

/// Parses a `rename_all` rule, honoring the deserialize/serialize split.
fn parse_rule(
    meta: &syn::meta::ParseNestedMeta<'_>,
    deserialize_target: &mut Option<RenameRule>,
    serialize_target: &mut Option<RenameRule>,
) -> syn::Result<()> {
    if meta.input.peek(syn::Token![=]) {
        if deserialize_target.is_some() || serialize_target.is_some() {
            return Err(meta.error("duplicate serde rename rule"));
        }
        let rule = RenameRule::parse(&meta.value()?.parse::<LitStr>()?)?;
        *deserialize_target = Some(rule);
        *serialize_target = Some(rule);
        return Ok(());
    }
    meta.parse_nested_meta(|nested| {
        if nested.path.is_ident("deserialize") {
            if deserialize_target.is_some() {
                return Err(nested.error("duplicate serde deserialize rename rule"));
            }
            *deserialize_target = Some(RenameRule::parse(&nested.value()?.parse::<LitStr>()?)?);
        } else if nested.path.is_ident("serialize") {
            if serialize_target.is_some() {
                return Err(nested.error("duplicate serde serialize rename rule"));
            }
            *serialize_target = Some(RenameRule::parse(&nested.value()?.parse::<LitStr>()?)?);
        } else {
            return Err(nested.error("expected `serialize` or `deserialize`"));
        }
        Ok(())
    })
}

/// Parses `default` / `default = "path"`.
fn parse_default(meta: &syn::meta::ParseNestedMeta<'_>) -> syn::Result<DefaultValue> {
    if meta.input.peek(syn::Token![=]) {
        let value = meta.value()?.parse::<LitStr>()?;
        Ok(DefaultValue::Path(value.parse()?))
    } else {
        Ok(DefaultValue::Trait)
    }
}

/// Parses the field attributes on `attrs`.
pub(crate) fn parse_field(attrs: &[Attribute]) -> syn::Result<FieldAttrs> {
    let mut result = FieldAttrs::default();
    for attr in attrs {
        if attr.path().is_ident("internity") {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("via_serde") {
                    if result.via_serde {
                        return Err(meta.error("duplicate `via_serde`"));
                    }
                    result.via_serde = true;
                    Ok(())
                } else {
                    Err(meta.error("unsupported internity field attribute; expected `via_serde`"))
                }
            })?;
        } else if attr.path().is_ident("serde") {
            attr.parse_nested_meta(|meta| parse_serde_field_meta(&meta, &mut result))?;
        }
    }

    Ok(result)
}

fn parse_serde_field_meta(meta: &syn::meta::ParseNestedMeta<'_>, result: &mut FieldAttrs) -> syn::Result<()> {
    if meta.path.is_ident("rename") {
        parse_rename(meta, &mut result.rename, &mut result.serialize_rename)
    } else if meta.path.is_ident("alias") {
        result.aliases.push(meta.value()?.parse::<LitStr>()?.value());
        Ok(())
    } else if meta.path.is_ident("default") {
        if result.default.is_some() {
            return Err(meta.error("duplicate default"));
        }
        result.default = Some(parse_default(meta)?);
        Ok(())
    } else if meta.path.is_ident("skip") {
        result.skip = true;
        result.skip_serializing = true;
        Ok(())
    } else if meta.path.is_ident("skip_deserializing") {
        result.skip = true;
        Ok(())
    } else if meta.path.is_ident("deserialize_with") {
        set_with(result, meta.value()?.parse::<LitStr>()?.parse()?, meta)
    } else if meta.path.is_ident("with") {
        let mut path: Path = meta.value()?.parse::<LitStr>()?.parse()?;
        let mut serialize_path = path.clone();
        path.segments.push(syn::parse_quote!(deserialize));
        serialize_path.segments.push(syn::parse_quote!(serialize));
        set_serialize_with(result, serialize_path, meta)?;
        set_with(result, path, meta)
    } else if meta.path.is_ident("flatten") {
        Err(meta.error("serde `flatten` is not supported by internity's `DeserializeIn`"))
    } else if meta.path.is_ident("borrow") {
        Err(meta.error("serde `borrow` is not supported by internity's `DeserializeIn`"))
    } else if meta.path.is_ident("skip_serializing") {
        result.skip_serializing = true;
        Ok(())
    } else if meta.path.is_ident("serialize_with") {
        set_serialize_with(result, meta.value()?.parse::<LitStr>()?.parse()?, meta)
    } else if meta.path.is_ident("skip_serializing_if") {
        // Parsed so `DeserializeIn` still works, but `SerializeIn` cannot honor a
        // runtime skip predicate without diverging from the type's ordinary Serde
        // wire schema, so serialize expansion rejects it (see `expand_serialize_*`).
        let _ = meta.value()?.parse::<LitStr>()?;
        result.skip_serializing_if = true;
        Ok(())
    } else if meta.path.is_ident("getter") {
        // `getter` is only valid together with `#[serde(remote = "...")]`, which
        // internity does not support. Accepting it would silently change the
        // declared schema, so reject it explicitly.
        Err(meta.error("serde `getter` is only valid with `remote`, which internity's derives do not support"))
    } else {
        Err(meta.error("unsupported serde field attribute for internity's `DeserializeIn`"))
    }
}

fn set_with(result: &mut FieldAttrs, path: Path, meta: &syn::meta::ParseNestedMeta<'_>) -> syn::Result<()> {
    if result.with.is_some() {
        return Err(meta.error("duplicate serde deserializer"));
    }
    result.with = Some(path);
    Ok(())
}

fn set_serialize_with(result: &mut FieldAttrs, path: Path, meta: &syn::meta::ParseNestedMeta<'_>) -> syn::Result<()> {
    if result.serialize_with.is_some() {
        return Err(meta.error("duplicate serde serializer"));
    }
    result.serialize_with = Some(path);
    Ok(())
}

/// Parses the container attributes on `attrs`.
pub(crate) fn parse_container(attrs: &[Attribute]) -> syn::Result<ContainerAttrs> {
    let mut result = ContainerAttrs::default();
    for attr in attrs {
        if attr.path().is_ident("internity") {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("crate") {
                    if result.internity_crate.is_some() {
                        return Err(meta.error("duplicate `internity(crate = ...)`"));
                    }
                    result.internity_crate = Some(meta.value()?.parse::<LitStr>()?.parse()?);
                    Ok(())
                } else {
                    Err(meta.error("unsupported internity container attribute; expected `crate = \"...\"`"))
                }
            })?;
        } else if attr.path().is_ident("serde") {
            attr.parse_nested_meta(|meta| parse_serde_container_meta(&meta, &mut result))?;
        }
    }
    Ok(result)
}

fn parse_serde_container_meta(meta: &syn::meta::ParseNestedMeta<'_>, result: &mut ContainerAttrs) -> syn::Result<()> {
    if meta.path.is_ident("rename_all") {
        parse_rule(meta, &mut result.rename_all, &mut result.serialize_rename_all)
    } else if meta.path.is_ident("rename") {
        parse_rename(meta, &mut result.rename, &mut result.serialize_rename)
    } else if meta.path.is_ident("deny_unknown_fields") {
        result.deny_unknown_fields = true;
        Ok(())
    } else if meta.path.is_ident("default") {
        if result.default.is_some() {
            return Err(meta.error("duplicate serde container default"));
        }
        result.default = Some(parse_default(meta)?);
        Ok(())
    } else if meta.path.is_ident("transparent") {
        if result.transparent {
            return Err(meta.error("duplicate serde `transparent`"));
        }
        result.transparent = true;
        Ok(())
    } else if meta.path.is_ident("expecting") {
        if result.expecting.is_some() {
            return Err(meta.error("duplicate serde container expectation"));
        }
        result.expecting = Some(meta.value()?.parse::<LitStr>()?.value());
        Ok(())
    } else if meta.path.is_ident("bound") {
        // internity forbids generics, so deserialize bounds have no effect; accept
        // and ignore for parity with plain `serde::Deserialize`.
        consume_bound(meta)
    } else if meta.path.is_ident("rename_all_fields") {
        // Enum-only; internity supports only structs.
        Err(meta.error("serde `rename_all_fields` applies to enums, which internity's `DeserializeIn` does not support"))
    } else if meta.path.is_ident("from") {
        // Deserialize-only conversion: record its span and let the deserialize
        // expander reject it, while the serialize expander ignores it.
        result.from = Some(record_conversion(meta)?);
        Ok(())
    } else if meta.path.is_ident("try_from") {
        result.try_from = Some(record_conversion(meta)?);
        Ok(())
    } else if meta.path.is_ident("into") {
        // Serialize-only conversion: recorded here, rejected by the serialize
        // expander and ignored by the deserialize expander.
        result.into = Some(record_conversion(meta)?);
        Ok(())
    } else if is_rejected_container_repr(meta) {
        Err(meta.error("this serde container representation is not supported by internity's derives"))
    } else {
        Err(meta.error("unsupported serde container attribute for internity's derives"))
    }
}

/// Consumes the `= "Type"` value of a `from`/`try_from`/`into` conversion and
/// returns its span for later direction-specific diagnostics.
fn record_conversion(meta: &syn::meta::ParseNestedMeta<'_>) -> syn::Result<proc_macro2::Span> {
    let span = meta.path.span();
    let _ = meta.value()?.parse::<LitStr>()?;
    Ok(span)
}

fn is_rejected_container_repr(meta: &syn::meta::ParseNestedMeta<'_>) -> bool {
    ["tag", "content", "untagged", "remote"].iter().any(|name| meta.path.is_ident(name))
}

fn consume_bound(meta: &syn::meta::ParseNestedMeta<'_>) -> syn::Result<()> {
    if meta.input.peek(syn::Token![=]) {
        let _ = meta.value()?.parse::<LitStr>()?;
        return Ok(());
    }
    meta.parse_nested_meta(|nested| {
        let _ = nested.value()?.parse::<LitStr>()?;
        Ok(())
    })
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use proc_macro2::Span;
    use syn::{Data, DeriveInput, Field, LitStr, parse_quote};

    use super::{ContainerAttrs, DefaultValue, FieldAttrs, RenameRule, parse_container, parse_field};

    /// Asserts a parse result is an error without requiring the `Ok` value to be
    /// `Debug` (as `Result::expect_err` would), keeping the parsed attribute
    /// structs free of an otherwise-unexercised derived `Debug`.
    trait AssertIsErr {
        fn assert_is_err(self, msg: &str);
        fn assert_err_contains(self, needle: &str);
    }

    impl<T> AssertIsErr for syn::Result<T> {
        fn assert_is_err(self, msg: &str) {
            assert!(self.is_err(), "{msg}");
        }

        fn assert_err_contains(self, needle: &str) {
            let message = self.err().map(|err| err.to_string()).unwrap_or_default();
            assert!(message.contains(needle), "error {message:?} should mention {needle:?}");
        }
    }

    fn field_attrs(field: Field) -> syn::Result<FieldAttrs> {
        let attrs = field.attrs;
        parse_field(&attrs)
    }

    fn container_attrs(input: DeriveInput) -> syn::Result<ContainerAttrs> {
        let attrs = input.attrs;
        parse_container(&attrs)
    }

    #[test]
    fn rename_rule_parse_accepts_every_documented_casing() {
        for casing in [
            "lowercase",
            "UPPERCASE",
            "PascalCase",
            "camelCase",
            "snake_case",
            "SCREAMING_SNAKE_CASE",
            "kebab-case",
            "SCREAMING-KEBAB-CASE",
        ] {
            RenameRule::parse(&LitStr::new(casing, Span::call_site())).unwrap();
        }
        RenameRule::parse(&LitStr::new("nonsense", Span::call_site())).assert_is_err("unknown casing must be rejected");
    }

    #[test]
    fn rename_rule_apply_transforms_each_variant() {
        assert_eq!(RenameRule::Lower.apply("long_name"), "long_name");
        assert_eq!(RenameRule::Snake.apply("long_name"), "long_name");
        assert_eq!(RenameRule::Upper.apply("long_name"), "LONG_NAME");
        assert_eq!(RenameRule::ScreamingSnake.apply("long_name"), "LONG_NAME");
        assert_eq!(RenameRule::Pascal.apply("long_name"), "LongName");
        assert_eq!(RenameRule::Camel.apply("long_name"), "longName");
        assert_eq!(RenameRule::Kebab.apply("long_name"), "long-name");
        assert_eq!(RenameRule::ScreamingKebab.apply("long_name"), "LONG-NAME");
        // Empty segments (a leading underscore yields one) exercise the
        // `capitalize`/`lowercase_first` empty-string branches.
        assert_eq!(RenameRule::Pascal.apply("_leading"), "Leading");
        assert_eq!(RenameRule::Camel.apply(""), "");
    }

    #[test]
    fn field_via_serde_and_its_errors() {
        assert!(field_attrs(parse_quote!(#[internity(via_serde)] a: Sym)).unwrap().via_serde);
        field_attrs(parse_quote!(#[internity(via_serde)] #[internity(via_serde)] a: Sym)).assert_is_err("duplicate via_serde must fail");
        field_attrs(parse_quote!(#[internity(bogus)] a: Sym)).assert_is_err("unknown internity field attribute must fail");
    }

    #[test]
    fn field_rename_variants() {
        let both = field_attrs(parse_quote!(#[serde(rename = "x")] a: Sym)).unwrap();
        assert_eq!(both.rename.as_deref(), Some("x"));
        assert_eq!(both.serialize_rename.as_deref(), Some("x"));

        let split = field_attrs(parse_quote!(#[serde(rename(deserialize = "d", serialize = "s"))] a: Sym)).unwrap();
        assert_eq!(split.rename.as_deref(), Some("d"));
        assert_eq!(split.serialize_rename.as_deref(), Some("s"));
        // serialize-only split leaves the deserialize name unset.
        let serialize_only = field_attrs(parse_quote!(#[serde(rename(serialize = "s"))] a: Sym)).unwrap();
        assert!(serialize_only.rename.is_none());
        assert_eq!(serialize_only.serialize_rename.as_deref(), Some("s"));
        field_attrs(parse_quote!(#[serde(rename = "a", rename = "b")] a: Sym)).assert_is_err("duplicate rename must fail");
        field_attrs(parse_quote!(#[serde(rename(deserialize = "a", deserialize = "b"))] a: Sym))
            .assert_is_err("duplicate deserialize rename must fail");
        field_attrs(parse_quote!(#[serde(rename(serialize = "a", serialize = "b"))] a: Sym))
            .assert_is_err("duplicate serialize rename must fail");
        field_attrs(parse_quote!(#[serde(rename(unknown = "a"))] a: Sym)).assert_is_err("unknown rename target must fail");
        field_attrs(parse_quote!(#[serde(rename = 5)] a: Sym)).assert_is_err("invalid rename literal must fail");
        field_attrs(parse_quote!(#[serde(rename(deserialize = 5))] a: Sym)).assert_is_err("invalid deserialize rename literal must fail");
        field_attrs(parse_quote!(#[serde(rename(serialize = 5))] a: Sym)).assert_is_err("invalid serialize rename literal must fail");
    }

    #[test]
    fn field_alias_default_and_skip() {
        let aliased = field_attrs(parse_quote!(#[serde(alias = "one", alias = "two")] a: Sym)).unwrap();
        assert_eq!(aliased.aliases, vec!["one".to_owned(), "two".to_owned()]);

        assert!(matches!(
            field_attrs(parse_quote!(#[serde(default)] a: Sym)).unwrap().default,
            Some(DefaultValue::Trait)
        ));
        assert!(matches!(
            field_attrs(parse_quote!(#[serde(default = "make")] a: Sym)).unwrap().default,
            Some(DefaultValue::Path(_))
        ));
        field_attrs(parse_quote!(#[serde(default, default)] a: Sym)).assert_is_err("duplicate default must fail");
        field_attrs(parse_quote!(#[serde(alias = 5)] a: Sym)).assert_is_err("invalid alias literal must fail");
        field_attrs(parse_quote!(#[serde(default = 5)] a: Sym)).assert_is_err("invalid default literal must fail");
        field_attrs(parse_quote!(#[serde(default = "123")] a: Sym)).assert_is_err("invalid default path must fail");

        let skip = field_attrs(parse_quote!(#[serde(skip)] a: Sym)).unwrap();
        assert!(skip.skip);
        assert!(skip.skip_serializing);
        assert!(field_attrs(parse_quote!(#[serde(skip_deserializing)] a: Sym)).unwrap().skip);
        assert!(
            field_attrs(parse_quote!(#[serde(skip_serializing)] a: Sym))
                .unwrap()
                .skip_serializing
        );
    }

    #[test]
    fn field_with_deserialize_with_and_conflicts() {
        assert!(
            field_attrs(parse_quote!(#[serde(deserialize_with = "f")] a: Sym))
                .unwrap()
                .with
                .is_some()
        );
        let with = field_attrs(parse_quote!(#[serde(with = "m")] a: Sym)).unwrap();
        assert!(with.with.is_some());
        assert!(with.serialize_with.is_some());
        field_attrs(parse_quote!(#[serde(with = "m", deserialize_with = "f")] a: Sym)).assert_is_err("two custom deserializers must fail");
        field_attrs(parse_quote!(#[serde(deserialize_with = 5)] a: Sym)).assert_is_err("invalid deserialize_with literal must fail");
        field_attrs(parse_quote!(#[serde(deserialize_with = "123")] a: Sym)).assert_is_err("invalid deserialize_with path must fail");
        field_attrs(parse_quote!(#[serde(with = 5)] a: Sym)).assert_is_err("invalid with literal must fail");
        field_attrs(parse_quote!(#[serde(with = "123")] a: Sym)).assert_is_err("invalid with path must fail");
        // Direction-specific mode conflicts (`via_serde` vs a custom deserializer,
        // and `skip` vs a custom deserializer) are no longer rejected by the shared
        // parser: they are enforced per-direction by the deserialize/serialize
        // expanders so a valid serialization schema is not rejected. The parser only
        // records the attributes here.
        let via_serde_with = field_attrs(parse_quote!(#[internity(via_serde)] #[serde(with = "m")] a: Sym)).unwrap();
        assert!(via_serde_with.via_serde);
        assert!(via_serde_with.with.is_some());
        assert!(via_serde_with.serialize_with.is_some());
        let skip_with = field_attrs(parse_quote!(#[serde(skip, with = "m")] a: Sym)).unwrap();
        assert!(skip_with.skip);
        assert!(skip_with.with.is_some());
    }

    #[test]
    fn field_serialize_attrs_are_captured_and_unknowns_rejected() {
        assert!(
            field_attrs(parse_quote!(#[serde(skip_serializing)] a: Sym))
                .unwrap()
                .skip_serializing
        );
        field_attrs(parse_quote!(#[serde(skip_serializing_if = "Option::is_none")] a: Sym)).unwrap();
        assert!(
            field_attrs(parse_quote!(#[serde(serialize_with = "s")] a: Sym))
                .unwrap()
                .serialize_with
                .is_some()
        );
        field_attrs(parse_quote!(#[serde(serialize_with = "a", serialize_with = "b")] a: Sym))
            .assert_is_err("duplicate serializer must fail");
        field_attrs(parse_quote!(#[serde(serialize_with = 5)] a: Sym)).assert_is_err("invalid serialize_with literal must fail");
        field_attrs(parse_quote!(#[serde(serialize_with = "123")] a: Sym)).assert_is_err("invalid serialize_with path must fail");
        field_attrs(parse_quote!(#[serde(skip_serializing_if = 5)] a: Sym)).assert_is_err("invalid skip_serializing_if literal must fail");
        field_attrs(parse_quote!(#[serde(getter = "g")] a: Sym)).assert_is_err("getter is only valid with remote, which internity rejects");
        // Representations we cannot honor are rejected outright.
        field_attrs(parse_quote!(#[serde(flatten)] a: Sym)).assert_is_err("flatten must be rejected");
        field_attrs(parse_quote!(#[serde(borrow)] a: Sym)).assert_is_err("borrow must be rejected");
        field_attrs(parse_quote!(#[serde(unknown)] a: Sym)).assert_is_err("unknown field attribute must fail");
    }

    #[test]
    fn container_internity_crate() {
        let ok = container_attrs(parse_quote!(
            #[internity(crate = "renamed")]
            struct S;
        ))
        .unwrap();
        assert!(ok.internity_crate.is_some());
        container_attrs(parse_quote!(
            #[internity(crate = "a")]
            #[internity(crate = "b")]
            struct S;
        ))
        .assert_is_err("duplicate crate must fail");
        container_attrs(parse_quote!(
            #[internity(bogus)]
            struct S;
        ))
        .assert_is_err("unknown internity container attribute must fail");
        container_attrs(parse_quote!(
            #[internity(crate = 5)]
            struct S;
        ))
        .assert_is_err("invalid internity crate literal must fail");
        container_attrs(parse_quote!(
            #[internity(crate = "123")]
            struct S;
        ))
        .assert_is_err("invalid internity crate path must fail");
    }

    #[test]
    fn container_rename_all_and_rename() {
        let both = container_attrs(parse_quote!(
            #[serde(rename_all = "camelCase")]
            struct S;
        ))
        .unwrap();
        assert!(both.rename_all.is_some());
        assert!(both.serialize_rename_all.is_some());
        let split = container_attrs(parse_quote!(
            #[serde(rename_all(deserialize = "snake_case", serialize = "kebab-case"))]
            struct S;
        ))
        .unwrap();
        assert!(split.rename_all.is_some());
        assert!(split.serialize_rename_all.is_some());
        let serialize_only = container_attrs(parse_quote!(
            #[serde(rename_all(serialize = "kebab-case"))]
            struct S;
        ))
        .unwrap();
        assert!(serialize_only.rename_all.is_none());
        assert!(serialize_only.serialize_rename_all.is_some());
        container_attrs(parse_quote!(
            #[serde(rename_all = "snake_case", rename_all = "kebab-case")]
            struct S;
        ))
        .assert_is_err("duplicate rename_all must fail");
        container_attrs(parse_quote!(
            #[serde(rename_all(deserialize = "snake_case", deserialize = "kebab-case"))]
            struct S;
        ))
        .assert_is_err("duplicate deserialize rename_all must fail");
        container_attrs(parse_quote!(
            #[serde(rename_all(serialize = "snake_case", serialize = "kebab-case"))]
            struct S;
        ))
        .assert_is_err("duplicate serialize rename_all must fail");
        container_attrs(parse_quote!(
            #[serde(rename_all(unknown = "a"))]
            struct S;
        ))
        .assert_is_err("unknown rename_all target must fail");
        container_attrs(parse_quote!(
            #[serde(rename_all = "nope")]
            struct S;
        ))
        .assert_is_err("unknown rename_all rule must fail");
        container_attrs(parse_quote!(
            #[serde(rename_all = 5)]
            struct S;
        ))
        .assert_is_err("invalid rename_all literal must fail");
        container_attrs(parse_quote!(
            #[serde(rename_all(deserialize = 5))]
            struct S;
        ))
        .assert_is_err("invalid deserialize rename_all literal must fail");
        container_attrs(parse_quote!(
            #[serde(rename_all(serialize = 5))]
            struct S;
        ))
        .assert_is_err("invalid serialize rename_all literal must fail");
        let renamed = container_attrs(parse_quote!(
            #[serde(rename = "Wire")]
            struct S;
        ))
        .unwrap();
        assert_eq!(renamed.rename.as_deref(), Some("Wire"));
        assert_eq!(renamed.serialize_rename.as_deref(), Some("Wire"));
        let split_renamed = container_attrs(parse_quote!(
            #[serde(rename(deserialize = "Read", serialize = "Write"))]
            struct S;
        ))
        .unwrap();
        assert_eq!(split_renamed.rename.as_deref(), Some("Read"));
        assert_eq!(split_renamed.serialize_rename.as_deref(), Some("Write"));
        container_attrs(parse_quote!(
            #[serde(rename = "A", rename = "B")]
            struct S;
        ))
        .assert_is_err("duplicate container rename must fail");
        container_attrs(parse_quote!(
            #[serde(rename = 5)]
            struct S;
        ))
        .assert_is_err("invalid container rename literal must fail");
    }

    #[test]
    fn container_flags_and_defaults() {
        assert!(
            container_attrs(parse_quote!(
                #[serde(deny_unknown_fields)]
                struct S;
            ))
            .unwrap()
            .deny_unknown_fields
        );

        assert!(matches!(
            container_attrs(parse_quote!(
                #[serde(default)]
                struct S;
            ))
            .unwrap()
            .default,
            Some(DefaultValue::Trait)
        ));
        assert!(matches!(
            container_attrs(parse_quote!(
                #[serde(default = "make")]
                struct S;
            ))
            .unwrap()
            .default,
            Some(DefaultValue::Path(_))
        ));
        container_attrs(parse_quote!(
            #[serde(default, default)]
            struct S;
        ))
        .assert_is_err("duplicate container default must fail");
        container_attrs(parse_quote!(
            #[serde(default = 5)]
            struct S;
        ))
        .assert_is_err("invalid container default literal must fail");
        container_attrs(parse_quote!(
            #[serde(default = "123")]
            struct S;
        ))
        .assert_is_err("invalid container default path must fail");

        assert!(
            container_attrs(parse_quote!(
                #[serde(transparent)]
                struct S;
            ))
            .unwrap()
            .transparent
        );
        container_attrs(parse_quote!(
            #[serde(transparent, transparent)]
            struct S;
        ))
        .assert_is_err("duplicate transparent must fail");

        assert_eq!(
            container_attrs(parse_quote!(
                #[serde(expecting = "a widget")]
                struct S;
            ))
            .unwrap()
            .expecting
            .as_deref(),
            Some("a widget")
        );
        container_attrs(parse_quote!(
            #[serde(expecting = "a", expecting = "b")]
            struct S;
        ))
        .assert_is_err("duplicate expecting must fail");
        container_attrs(parse_quote!(
            #[serde(expecting = 5)]
            struct S;
        ))
        .assert_is_err("invalid expecting literal must fail");
    }

    #[test]
    fn container_bound_is_accepted_and_ignored() {
        container_attrs(parse_quote!(
            #[serde(bound = "T: Trait")]
            struct S;
        ))
        .unwrap();
        container_attrs(parse_quote!(
            #[serde(bound = 5)]
            struct S;
        ))
        .assert_is_err("invalid bound literal must fail");
        container_attrs(parse_quote!(
            #[serde(bound(deserialize = "T: Trait"))]
            struct S;
        ))
        .unwrap();
        container_attrs(parse_quote!(
            #[serde(bound(deserialize = 5))]
            struct S;
        ))
        .assert_is_err("invalid nested bound literal must fail");
    }

    #[test]
    fn container_rejected_representations() {
        container_attrs(parse_quote!(
            #[serde(rename_all_fields = "camelCase")]
            struct S;
        ))
        .assert_is_err("rename_all_fields is enum-only");
        for repr in [
            quote_repr(parse_quote!(
                #[serde(tag = "type")]
                struct S;
            )),
            quote_repr(parse_quote!(
                #[serde(content = "c")]
                struct S;
            )),
            quote_repr(parse_quote!(
                #[serde(untagged)]
                struct S;
            )),
            quote_repr(parse_quote!(
                #[serde(remote = "R")]
                struct S;
            )),
        ] {
            repr.assert_err_contains("representation is not supported");
        }
        // `from`/`try_from`/`into` are direction-specific conversions: they parse
        // successfully and are rejected only by the affected direction's expander.
        assert!(
            container_attrs(parse_quote!(
                #[serde(from = "R")]
                struct S;
            ))
            .unwrap()
            .from
            .is_some()
        );
        assert!(
            container_attrs(parse_quote!(
                #[serde(try_from = "R")]
                struct S;
            ))
            .unwrap()
            .try_from
            .is_some()
        );
        assert!(
            container_attrs(parse_quote!(
                #[serde(into = "R")]
                struct S;
            ))
            .unwrap()
            .into
            .is_some()
        );
        container_attrs(parse_quote!(
            #[serde(unknown_container_attr)]
            struct S;
        ))
        .assert_err_contains("unsupported serde container attribute");
    }

    fn quote_repr(input: DeriveInput) -> syn::Result<ContainerAttrs> {
        container_attrs(input)
    }

    #[test]
    fn field_helpers_reach_named_fields() {
        // Guards the `field_attrs` helper stays wired to the first named field.
        let input: DeriveInput = parse_quote! {
            struct S { #[serde(rename = "wire")] value: Sym }
        };
        let Data::Struct(data) = &input.data else { unreachable!() };
        let first = data.fields.iter().next().unwrap();
        assert_eq!(parse_field(&first.attrs).unwrap().rename.as_deref(), Some("wire"));
    }

    #[test]
    fn plain_rename_after_a_one_sided_split_is_a_duplicate() {
        // A split `rename`/`rename_all` that sets only one side, followed by a
        // plain form, must still be rejected as a duplicate. Guards the `||` in
        // the duplicate checks of `parse_rename` and `parse_rule` (an `&&` there
        // would silently accept the second, one-sided-overlapping form).
        field_attrs(parse_quote!(#[serde(rename(serialize = "s"), rename = "b")] a: Sym))
            .assert_is_err("plain rename after a serialize-only rename must fail");
        field_attrs(parse_quote!(#[serde(rename(deserialize = "d"), rename = "b")] a: Sym))
            .assert_is_err("plain rename after a deserialize-only rename must fail");
        container_attrs(parse_quote!(
            #[serde(rename_all(serialize = "kebab-case"), rename_all = "camelCase")]
            struct S;
        ))
        .assert_is_err("plain rename_all after a serialize-only rename_all must fail");
        container_attrs(parse_quote!(
            #[serde(rename_all(deserialize = "snake_case"), rename_all = "camelCase")]
            struct S;
        ))
        .assert_is_err("plain rename_all after a deserialize-only rename_all must fail");
    }
}
