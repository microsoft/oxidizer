// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Shared field-level attribute parsing for `#[dimension(...)]`, `#[unredacted]`,
//! and `#[data_class(...)]`.
//!
//! Both `#[derive(Event)]` and `#[derive(Enrichment)]` accept these attributes on fields.
//! This module provides the types and parsing logic so neither derive macro duplicates it.

use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{Attribute, Error, Expr, Ident, LitStr, Meta, Result, Token};

/// Log-signal routing for a field, controlled by the `#[dimension(...)]`
/// attribute's `log` setting.
#[derive(Default)]
pub(crate) enum LogRouting {
    /// Logged under the field's own name. This is the default both when no
    /// `#[dimension(...)]` attribute is present and when the attribute sets a
    /// metric key without touching the log signal.
    #[default]
    Default,
    /// Logged under an explicit key.
    Rename(String),
    /// Excluded from the log signal.
    Exclude,
}

/// Metric-dimension routing for a field, controlled by the `#[dimension(...)]`
/// attribute's `metric` setting.
#[derive(Default)]
pub(crate) enum MetricRouting {
    /// The field is not a metric dimension.
    #[default]
    None,
    /// The field is a metric dimension keyed by the field's own name (bare `metric`).
    OwnName,
    /// The field is a metric dimension keyed by an explicit name (`metric = "..."`).
    Named(String),
}

impl MetricRouting {
    /// Returns whether the field participates in the metric signal as a dimension.
    pub(crate) fn is_dimension(&self) -> bool {
        !matches!(self, Self::None)
    }

    /// Resolves the dimension key, or `None` when the field is not a metric dimension.
    /// `own_name` supplies the key for the bare `metric` (own-name) opt-in.
    pub(crate) fn resolve_key(&self, own_name: &str) -> Option<String> {
        match self {
            Self::None => None,
            Self::OwnName => Some(own_name.to_owned()),
            Self::Named(name) => Some(name.clone()),
        }
    }
}

/// Parsed `#[dimension(...)]` configuration, unifying log and metric routing.
///
/// A field is a log attribute unless [`LogRouting::Exclude`] is set, and a metric
/// dimension only when [`metric`](Self::metric) is not [`MetricRouting::None`].
pub(crate) struct Dimension {
    pub log: LogRouting,
    pub metric: MetricRouting,
}

/// A single comma-separated item inside `#[dimension(...)]`.
enum DimensionItem {
    /// `log = "name"`.
    LogRename(String),
    /// `log = exclude`.
    LogExclude,
    /// `metric = "name"`.
    MetricNamed(String),
    /// bare `metric`: opt in as a metric dimension keyed by the field's own name.
    MetricOwnName,
}

impl Parse for DimensionItem {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        if input.peek(LitStr) {
            let lit = input.parse::<LitStr>()?;
            return Err(Error::new_spanned(
                lit,
                "a positional string name is not supported; use `#[dimension(log = \"...\")]` \
                 to rename the log key",
            ));
        }

        let key: Ident = input.parse()?;
        if key == "log" {
            input.parse::<Token![=]>()?;
            if input.peek(LitStr) {
                return Ok(Self::LogRename(input.parse::<LitStr>()?.value()));
            }
            let value: Ident = input.parse()?;
            return if value == "exclude" {
                Ok(Self::LogExclude)
            } else {
                Err(Error::new_spanned(value, "`log` expects a string key or the `exclude` keyword"))
            };
        }
        if key == "metric" {
            // `metric` is a bare opt-in (own-name key); `metric = "..."` names the key.
            if input.peek(Token![=]) {
                input.parse::<Token![=]>()?;
                return Ok(Self::MetricNamed(input.parse::<LitStr>()?.value()));
            }
            return Ok(Self::MetricOwnName);
        }

        Err(Error::new_spanned(
            key,
            "expected `log = \"...\"`, `log = exclude`, `metric`, or `metric = \"...\"`",
        ))
    }
}

/// Parses a `#[dimension(...)]` attribute into a [`Dimension`].
fn parse_dimension(attr: &Attribute) -> Result<Dimension> {
    match &attr.meta {
        // Bare `#[dimension]` logs the field under its own name and opts out of the
        // metric signal - equivalent to the field's default routing, made explicit.
        Meta::Path(_) => Ok(Dimension {
            log: LogRouting::Default,
            metric: MetricRouting::None,
        }),
        Meta::NameValue(_) => Err(Error::new_spanned(
            attr,
            "`#[dimension]` does not take a `= value`; use `#[dimension]` or \
             `#[dimension(log = ..., metric = ...)]`",
        )),
        Meta::List(list) => {
            let items = list.parse_args_with(Punctuated::<DimensionItem, Token![,]>::parse_terminated)?;
            fold_dimension(attr, items.into_iter().collect())
        }
    }
}

/// Folds the parsed comma-separated items into a validated [`Dimension`].
///
/// A `log` item occupies the `log` slot and may be paired with a `metric`
/// opt-in (but not a second `log` item).
fn fold_dimension(attr: &Attribute, items: Vec<DimensionItem>) -> Result<Dimension> {
    if items.is_empty() {
        return Err(Error::new_spanned(
            attr,
            "`#[dimension(...)]` requires at least one of `log` or `metric`",
        ));
    }

    let mut log: Option<LogRouting> = None;
    let mut metric: Option<MetricRouting> = None;

    for item in items {
        match item {
            DimensionItem::LogRename(name) => {
                set_once(&mut log, LogRouting::Rename(name), attr, "log")?;
            }
            DimensionItem::LogExclude => {
                set_once(&mut log, LogRouting::Exclude, attr, "log")?;
            }
            DimensionItem::MetricNamed(name) => {
                set_once(&mut metric, MetricRouting::Named(name), attr, "metric")?;
            }
            DimensionItem::MetricOwnName => {
                set_once(&mut metric, MetricRouting::OwnName, attr, "metric")?;
            }
        }
    }

    Ok(Dimension {
        log: log.unwrap_or_default(),
        metric: metric.unwrap_or_default(),
    })
}

/// Assigns `value` to `slot` if unset, otherwise reports a duplicate-`what` error.
fn set_once<T>(slot: &mut Option<T>, value: T, attr: &Attribute, what: &str) -> Result<()> {
    if slot.is_some() {
        return Err(Error::new_spanned(
            attr,
            format!("duplicate `{what}` setting in `#[dimension(...)]`"),
        ));
    }
    *slot = Some(value);
    Ok(())
}

/// Redaction handling for a field.
#[derive(Default)]
pub(crate) enum FieldRedaction {
    #[default]
    Default,
    Unredacted,
    DataClass(Expr),
}

/// Behavior for an `Option<T>` field when it holds no value (`None`), controlled
/// by the `#[if_none(...)]` attribute.
///
/// The default (no attribute) is [`Fill`](Self::Fill) with `"n/a"`, so an absent
/// optional value is still recorded under a stable placeholder rather than being
/// silently dropped.
#[derive(Clone)]
pub(crate) enum IfNone {
    /// Omit the field entirely: no log attribute and no metric dimension are recorded.
    Drop,
    /// Record the given placeholder string in place of the missing value.
    Fill(String),
}

impl Default for IfNone {
    fn default() -> Self {
        Self::Fill("n/a".to_owned())
    }
}

/// Parses a `#[if_none(...)]` attribute into an [`IfNone`]. The argument is either
/// the `drop` keyword or a string-literal placeholder (`#[if_none("n/a")]`).
fn parse_if_none(attr: &Attribute) -> Result<IfNone> {
    let Meta::List(_) = &attr.meta else {
        return Err(Error::new_spanned(
            attr,
            "`#[if_none(...)]` requires an argument: `drop` or a string literal placeholder",
        ));
    };
    attr.parse_args_with(|input: ParseStream<'_>| {
        if input.peek(LitStr) {
            return Ok(IfNone::Fill(input.parse::<LitStr>()?.value()));
        }
        let key: Ident = input.parse()?;
        if key == "drop" {
            return Ok(IfNone::Drop);
        }
        Err(Error::new_spanned(
            key,
            "expected `drop` or a string literal placeholder, e.g. `#[if_none(\"n/a\")]`",
        ))
    })
}

/// Accumulates shared field attributes (`#[dimension]`, `#[unredacted]`, `#[data_class]`,
/// `#[if_none]`) parsed from a field's attribute list.
///
/// Call [`try_parse`](Self::try_parse) for each attribute on the field. It returns `Ok(true)` if
/// the attribute was handled, `Ok(false)` if it was not recognized (so the caller can try
/// macro-specific parsing).
#[derive(Default)]
pub(crate) struct SharedFieldAttrs {
    pub dimension: Option<Dimension>,
    pub redaction: Option<FieldRedaction>,
    pub if_none: Option<IfNone>,
}

impl SharedFieldAttrs {
    /// Try to parse a shared attribute. Returns `Ok(true)` if handled, `Ok(false)` if not
    /// recognized.
    pub(crate) fn try_parse(&mut self, attr: &Attribute) -> Result<bool> {
        if attr.path().is_ident("dimension") {
            if self.dimension.is_some() {
                return Err(Error::new_spanned(attr, "duplicate `#[dimension(...)]` attribute"));
            }
            self.dimension = Some(parse_dimension(attr)?);
            Ok(true)
        } else if attr.path().is_ident("if_none") {
            if self.if_none.is_some() {
                return Err(Error::new_spanned(attr, "duplicate `#[if_none(...)]` attribute"));
            }
            self.if_none = Some(parse_if_none(attr)?);
            Ok(true)
        } else if attr.path().is_ident("unredacted") {
            if self.redaction.is_some() {
                return Err(Error::new_spanned(
                    attr,
                    "`#[unredacted]` and `#[data_class(...)]` are mutually exclusive",
                ));
            }
            if let syn::Meta::List(_) = &attr.meta {
                return Err(Error::new_spanned(attr, "`#[unredacted]` does not accept arguments"));
            }
            self.redaction = Some(FieldRedaction::Unredacted);
            Ok(true)
        } else if attr.path().is_ident("data_class") {
            if self.redaction.is_some() {
                return Err(Error::new_spanned(
                    attr,
                    "`#[unredacted]` and `#[data_class(...)]` are mutually exclusive",
                ));
            }
            let expr: Expr = attr.parse_args()?;
            self.redaction = Some(FieldRedaction::DataClass(expr));
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

/// Strips transparent `Paren`/`Group` wrappers. Macro expansions wrap substituted
/// `$ty` fragments in `Type::Group` (invisible delimiter), so structural checks
/// must look through them.
fn unwrap_groups(ty: &syn::Type) -> &syn::Type {
    match ty {
        syn::Type::Paren(inner) => unwrap_groups(&inner.elem),
        syn::Type::Group(inner) => unwrap_groups(&inner.elem),
        other => other,
    }
}

/// Returns whether the (group-unwrapped) type is a reference.
pub(crate) fn is_reference_type(ty: &syn::Type) -> bool {
    matches!(unwrap_groups(ty), syn::Type::Reference(_))
}

/// Returns the inner type `T` if `ty` is syntactically `Option<T>`.
///
/// This is a purely syntactic match (the same approach `serde`/`clap`/`templated_uri`
/// use): it matches on the last path segment being `Option` with exactly one
/// generic type argument, so `Option<T>`, `std::option::Option<T>`, and
/// `core::option::Option<T>` are all recognized. A type aliased to `Option`
/// will not be detected.
pub(crate) fn option_inner_type(ty: &syn::Type) -> Option<&syn::Type> {
    let syn::Type::Path(type_path) = unwrap_groups(ty) else {
        return None;
    };
    if type_path.qself.is_some() {
        return None;
    }
    let last_segment = type_path.path.segments.last()?;
    if last_segment.ident != "Option" {
        return None;
    }
    let syn::PathArguments::AngleBracketed(args) = &last_segment.arguments else {
        return None;
    };
    if args.args.len() != 1 {
        return None;
    }
    match &args.args[0] {
        syn::GenericArgument::Type(inner_ty) => Some(inner_ty),
        _ => None,
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod coverage_tests {
    use super::*;

    fn ty(s: &str) -> syn::Type {
        syn::parse_str(s).expect("parse type")
    }

    #[test]
    fn option_inner_type_rejects_non_option_shapes() {
        // qualified self (`<T as Trait>::Assoc`).
        assert!(option_inner_type(&ty("<i32 as Copy>::Output")).is_none());
        // `Option` without angle-bracketed arguments.
        assert!(option_inner_type(&ty("Option")).is_none());
        // more than one generic argument.
        assert!(option_inner_type(&ty("Option<u8, u16>")).is_none());
        // a non-type (lifetime) generic argument.
        assert!(option_inner_type(&ty("Option<'a>")).is_none());
    }
}
