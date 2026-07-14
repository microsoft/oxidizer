// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Implementation of the `#[derive(Event)]` macro.
//!
//! Two phases:
//! 1. **Parse phase**: parse `DeriveInput` into intermediate structs.
//! 2. **Code generation phase**: generate a `TokenStream` from the parsed
//!    definitions.
//!
//! See the [`Event`](crate::Event) derive macro documentation for the full
//! attribute syntax reference.

use darling::FromMeta;
use proc_macro2::TokenStream;
use quote::{ToTokens, format_ident, quote};
use syn::{Data, DeriveInput, Error, Expr, Field, Fields, Generics, Ident, Result};

use crate::field_attrs::{
    Dimension, FieldRedaction, IfNone, LogRouting, MetricRouting, SharedFieldAttrs, is_reference_type, option_inner_type,
};

// ================================================================================================
// Attribute argument structs (parsed via `darling`)
// ================================================================================================

/// Parsed definition of an event struct.
struct EventDef {
    ident: Ident,
    /// The canonical event name from `#[event(name = "...")]`.
    event_name: String,
    generics: Generics,
    log: Option<LogArgs>,
    /// Event-level metric (a `#[metric(kind = counter)]` declared without a
    /// `field`), which records `1` per emission. Always a counter instrument.
    metric: Option<EventMetric>,
    disabled: bool,
    fields: Vec<FieldDef>,
}

/// `#[event(name = "...")]`.
#[derive(FromMeta)]
struct EventArgs {
    name: String,
}

/// `#[log(severity = <ident> [, name = "..."] [, message = "..."])]`.
#[derive(FromMeta)]
struct LogArgs {
    severity: SeverityKind,
    name: Option<String>,
    message: Option<String>,
}

/// Arguments accepted by the struct-level `#[metric(...)]` attribute.
///
/// `kind` selects the instrument type: `counter`, `updown_counter`, `gauge`, or
/// `histogram` (written as a bare identifier; case-sensitive).
///
/// `field` names the struct field whose value the instrument records. It is
/// optional for `kind = counter` (a fieldless counter records `1` per emission)
/// and required for every other instrument kind. It is written as a bare
/// identifier (`field = duration_ms`), not a string.
///
/// `name` overrides the instrument's metric name, which otherwise defaults to
/// the event name (`#[event(name = "...")]`). `description` and `unit` supply
/// the corresponding OpenTelemetry metadata.
#[derive(FromMeta)]
struct InstrumentArgs {
    kind: InstrumentKindValue,
    field: Option<Ident>,
    name: Option<String>,
    description: Option<String>,
    unit: Option<String>,
}

/// A struct-level instrument declaration before it is resolved against the
/// event's fields.
struct MetricSpec {
    args: InstrumentArgs,
    /// The instrument attribute, retained for error spans.
    attr: syn::Attribute,
}

/// Event-level metric metadata (fieldless counter, records `1` per emission).
struct EventMetric {
    name: Option<String>,
    description: Option<String>,
    unit: Option<String>,
}

/// Metric metadata resolved onto a single field. The field's value is the
/// measurement recorded for the instrument.
struct FieldMetric {
    name: String,
    kind: InstrumentKindValue,
    description: Option<String>,
    unit: Option<String>,
}

/// Parsed definition of a single field.
struct FieldDef {
    ident: Ident,
    ty: syn::Type,
    /// Log-signal routing for the field (logged by default under its own name).
    log: LogRouting,
    /// Metric-dimension routing for the field.
    metric_dimension: MetricRouting,
    /// `Some` when a struct-level instrument targets this field via `field = ...`.
    metric_value: Option<FieldMetric>,
    redaction: FieldRedaction,
    /// Behavior when an `Option<T>` field holds no value (`None`).
    if_none: IfNone,
}

impl FieldDef {
    /// Returns the field's log key, or `None` when the field is excluded from
    /// the log signal. A field is logged under its own name by default.
    fn log_key(&self) -> Option<String> {
        match &self.log {
            LogRouting::Default => Some(self.ident.to_string()),
            LogRouting::Rename(name) => Some(name.clone()),
            LogRouting::Exclude => None,
        }
    }
}

#[derive(Clone, Copy)]
enum InstrumentKindValue {
    Counter,
    UpDownCounter,
    Gauge,
    Histogram,
}

#[derive(Clone, Copy)]
enum SeverityKind {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
    Fatal,
}

impl FromMeta for SeverityKind {
    fn from_string(value: &str) -> darling::Result<Self> {
        match value {
            "trace" => Ok(Self::Trace),
            "debug" => Ok(Self::Debug),
            "info" => Ok(Self::Info),
            "warn" => Ok(Self::Warn),
            "error" => Ok(Self::Error),
            "fatal" => Ok(Self::Fatal),
            _ => Err(darling::Error::custom("expected one of: trace, debug, info, warn, error, fatal")),
        }
    }

    fn from_expr(expr: &Expr) -> darling::Result<Self> {
        ident_or_str(expr, Self::from_string)
    }
}

impl ToTokens for SeverityKind {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let ident = match self {
            Self::Trace => "Trace",
            Self::Debug => "Debug",
            Self::Info => "Info",
            Self::Warn => "Warn",
            Self::Error => "Error",
            Self::Fatal => "Fatal",
        };
        format_ident!("{ident}").to_tokens(tokens);
    }
}

impl FromMeta for InstrumentKindValue {
    fn from_string(value: &str) -> darling::Result<Self> {
        match value {
            "counter" => Ok(Self::Counter),
            "updown_counter" => Ok(Self::UpDownCounter),
            "gauge" => Ok(Self::Gauge),
            "histogram" => Ok(Self::Histogram),
            _ => Err(darling::Error::custom("expected one of: counter, updown_counter, gauge, histogram")),
        }
    }

    fn from_expr(expr: &Expr) -> darling::Result<Self> {
        ident_or_str(expr, Self::from_string)
    }
}

impl ToTokens for InstrumentKindValue {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        format_ident!("{}", self.variant_name()).to_tokens(tokens);
    }
}

impl InstrumentKindValue {
    /// The attribute spelling of the kind (`kind = <name>`), used in error
    /// messages. This is the lowercase `snake_case` form users write.
    fn attr_name(self) -> &'static str {
        match self {
            Self::Counter => "counter",
            Self::UpDownCounter => "updown_counter",
            Self::Gauge => "gauge",
            Self::Histogram => "histogram",
        }
    }

    /// The `InstrumentKind` enum variant name used in the generated
    /// `InstrumentKind::<name>` path.
    fn variant_name(self) -> &'static str {
        match self {
            Self::Counter => "Counter",
            Self::UpDownCounter => "UpDownCounter",
            Self::Gauge => "Gauge",
            Self::Histogram => "Histogram",
        }
    }
}

/// The signedness of a primitive integer type, used to enforce metric value
/// constraints (`kind = counter` requires unsigned, `kind = updown_counter`
/// requires signed).
#[derive(Clone, Copy, PartialEq, Eq)]
enum IntegerSignedness {
    Signed,
    Unsigned,
}

/// Returns the signedness of a primitive integer type, matched syntactically on
/// the last path segment (so `u64`, `std::primitive::u64` are recognized, but a
/// type aliased to an integer is not). Returns `None` for non-integer types
/// (e.g. `f64`) or unrecognized paths. `Option<T>`, group, and paren wrappers
/// are transparent.
fn integer_signedness(ty: &syn::Type) -> Option<IntegerSignedness> {
    let ty = option_inner_type(ty).unwrap_or(ty);
    let syn::Type::Path(type_path) = strip_type_wrappers(ty) else {
        return None;
    };
    let ident = type_path.path.segments.last()?.ident.to_string();
    match ident.as_str() {
        "u8" | "u16" | "u32" | "u64" | "u128" | "usize" => Some(IntegerSignedness::Unsigned),
        "i8" | "i16" | "i32" | "i64" | "i128" | "isize" => Some(IntegerSignedness::Signed),
        _ => None,
    }
}

/// Strips transparent `Paren`/`Group` wrappers from a type.
fn strip_type_wrappers(ty: &syn::Type) -> &syn::Type {
    match ty {
        syn::Type::Paren(inner) => strip_type_wrappers(&inner.elem),
        syn::Type::Group(inner) => strip_type_wrappers(&inner.elem),
        other => other,
    }
}

/// Parses an enum value written either as a bare identifier (`severity = info`)
/// or a string literal (`severity = "info"`). `darling`'s derived enum support
/// only accepts the string form, so both enums route their `from_expr` here.
fn ident_or_str<T>(expr: &Expr, parse: fn(&str) -> darling::Result<T>) -> darling::Result<T> {
    match expr {
        Expr::Path(p) if p.path.get_ident().is_some() => {
            parse(&p.path.get_ident().expect("match guard ensures a single-ident path").to_string()).map_err(|e| e.with_span(expr))
        }
        Expr::Lit(syn::ExprLit { lit: syn::Lit::Str(s), .. }) => parse(&s.value()).map_err(|e| e.with_span(expr)),
        _ => Err(darling::Error::unexpected_expr_type(expr)),
    }
}

/// Converts a `darling` parse error into a `syn` error spanned on the attribute,
/// preserving `darling`'s message so it surfaces through the derive's `Result`.
fn to_syn(err: &darling::Error, attr: &syn::Attribute) -> Error {
    Error::new_spanned(attr, err.to_string())
}

// ================================================================================================
// Parse phase
// ================================================================================================

fn parse_event_def(input: &DeriveInput) -> Result<EventDef> {
    let fields: Vec<&Field> = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(fields) => fields.named.iter().collect(),
            Fields::Unit => Vec::new(),
            Fields::Unnamed(_) => {
                return Err(Error::new_spanned(input, "Event can only be derived for structs with named fields"));
            }
        },
        Data::Enum(_) => {
            return Err(Error::new_spanned(input, "Event can only be derived for structs, not enums"));
        }
        Data::Union(_) => {
            return Err(Error::new_spanned(input, "Event can only be derived for structs, not unions"));
        }
    };

    let mut log = None;
    let mut metric_specs: Vec<MetricSpec> = Vec::new();
    let mut disabled = false;
    let mut event_name: Option<String> = None;

    for attr in &input.attrs {
        if attr.path().is_ident("event") {
            if event_name.is_some() {
                return Err(Error::new_spanned(attr, "duplicate `#[event(...)]` attribute"));
            }
            event_name = Some(EventArgs::from_meta(&attr.meta).map_err(|e| to_syn(&e, attr))?.name);
        } else if attr.path().is_ident("log") {
            if log.is_some() {
                return Err(Error::new_spanned(attr, "duplicate `#[log(...)]` attribute"));
            }
            log = Some(LogArgs::from_meta(&attr.meta).map_err(|e| to_syn(&e, attr))?);
        } else if attr.path().is_ident("metric") {
            // `#[metric(kind = ..., ...)]` always requires arguments (at least
            // `kind`); the bare word form is rejected by `darling`.
            let args = InstrumentArgs::from_meta(&attr.meta).map_err(|e| to_syn(&e, attr))?;
            metric_specs.push(MetricSpec { args, attr: attr.clone() });
        } else if attr.path().is_ident("disabled") {
            disabled = true;
        }
    }

    let mut field_defs = Vec::with_capacity(fields.len());
    for field in fields {
        field_defs.push(parse_field_def(field)?);
    }

    let event_name = event_name
        .ok_or_else(|| Error::new_spanned(input, "Event requires `#[event(name = \"...\")]` to declare a canonical event name"))?;

    let metric = resolve_metrics(metric_specs, &mut field_defs, &event_name)?;

    Ok(EventDef {
        ident: input.ident.clone(),
        event_name,
        generics: input.generics.clone(),
        log,
        metric,
        disabled,
        fields: field_defs,
    })
}

/// Resolves struct-level instrument declarations against the event's fields.
///
/// Each `#[metric(kind = ..., field = ...)]` attaches its instrument to the
/// named field (whose value becomes the measurement). A fieldless
/// `#[metric(kind = counter)]` becomes the event-level metric that records `1`
/// per emission. In both cases the instrument's metric name defaults to the
/// event name; an explicit `name = "..."` overrides it. Returns the
/// event-level metric, if any.
fn resolve_metrics(specs: Vec<MetricSpec>, fields: &mut [FieldDef], event_name: &str) -> Result<Option<EventMetric>> {
    let mut event_metric: Option<EventMetric> = None;

    for spec in specs {
        let MetricSpec { args, attr } = spec;
        let kind = args.kind;

        let Some(field_ident) = args.field.clone() else {
            // Fieldless instrument: only `kind = counter` is allowed, and it
            // records `1` per emission as the event-level metric.
            if !matches!(kind, InstrumentKindValue::Counter) {
                return Err(Error::new_spanned(
                    &attr,
                    format!(
                        "`#[metric(kind = {})]` requires `field = ...` naming the struct field \
                         that holds the metric value",
                        kind.attr_name(),
                    ),
                ));
            }
            if event_metric.is_some() {
                return Err(Error::new_spanned(
                    &attr,
                    "only one event-level metric (a fieldless `#[metric(kind = counter)]`) \
                     is allowed",
                ));
            }
            event_metric = Some(EventMetric {
                name: args.name,
                description: args.description,
                unit: args.unit,
            });
            continue;
        };

        let field = fields.iter_mut().find(|f| f.ident == field_ident).ok_or_else(|| {
            Error::new_spanned(
                &attr,
                format!(
                    "`#[metric(kind = {}, field = {field_ident})]` references field \
                         `{field_ident}`, which does not exist in the struct",
                    kind.attr_name(),
                ),
            )
        })?;

        if field.metric_value.is_some() {
            return Err(Error::new_spanned(
                &attr,
                format!("field `{field_ident}` already has a metric instrument"),
            ));
        }
        if field.metric_dimension.is_dimension() {
            return Err(Error::new_spanned(
                &attr,
                format!(
                    "field `{field_ident}` cannot be both a metric value and a metric dimension \
                     (`#[dimension(metric = ...)]`)",
                ),
            ));
        }

        enforce_value_type(kind, field, &attr)?;

        field.metric_value = Some(FieldMetric {
            name: args.name.unwrap_or_else(|| event_name.to_string()),
            kind,
            description: args.description,
            unit: args.unit,
        });
    }

    Ok(event_metric)
}

/// Enforces requirement that `kind = counter` value fields are unsigned integers
/// and `kind = updown_counter` value fields are signed integers. `gauge`/`histogram`
/// place no signedness constraint on the value type.
fn enforce_value_type(kind: InstrumentKindValue, field: &FieldDef, attr: &syn::Attribute) -> Result<()> {
    let required = match kind {
        InstrumentKindValue::Counter => IntegerSignedness::Unsigned,
        InstrumentKindValue::UpDownCounter => IntegerSignedness::Signed,
        InstrumentKindValue::Gauge | InstrumentKindValue::Histogram => return Ok(()),
    };

    if integer_signedness(&field.ty) == Some(required) {
        return Ok(());
    }

    let (word, examples) = match required {
        IntegerSignedness::Unsigned => ("unsigned", "u8, u16, u32, u64, u128, usize"),
        IntegerSignedness::Signed => ("signed", "i8, i16, i32, i64, i128, isize"),
    };
    Err(Error::new_spanned(
        attr,
        format!(
            "`#[metric(kind = {})]` requires field `{}` to be a {word} integer type ({examples})",
            kind.attr_name(),
            field.ident,
        ),
    ))
}

fn parse_field_def(field: &Field) -> Result<FieldDef> {
    let ident = field.ident.clone().expect("named fields should have identifiers");

    let mut shared = SharedFieldAttrs::default();

    for attr in &field.attrs {
        if shared.try_parse(attr)? {
            continue;
        }
        if attr.path().is_ident("metric") {
            return Err(Error::new_spanned(
                attr,
                format!(
                    "`#[metric(...)]` is a struct-level attribute; place it on the event struct \
                     and use `field = {ident}` to reference this field",
                ),
            ));
        }
    }

    let (log, metric_dimension) = match shared.dimension {
        Some(Dimension { log, metric }) => (log, metric),
        None => (LogRouting::Default, MetricRouting::None),
    };

    if shared.if_none.is_some() && option_inner_type(&field.ty).is_none() {
        return Err(Error::new_spanned(field, "`#[if_none(...)]` is only valid on `Option<T>` fields"));
    }

    Ok(FieldDef {
        ident,
        ty: field.ty.clone(),
        log,
        metric_dimension,
        metric_value: None,
        redaction: shared.redaction.unwrap_or_default(),
        if_none: shared.if_none.unwrap_or_default(),
    })
}

// ================================================================================================
// Code generation
// ================================================================================================

pub(crate) fn derive_event(input: &DeriveInput) -> Result<TokenStream> {
    let def = parse_event_def(input)?;
    validate_message_placeholders(&def)?;
    Ok(generate_event_impl(&def))
}

/// Validates that all `{placeholder}` references in the log message correspond to
/// existing log attribute names. The attribute name is the `#[dimension(log = "...")]`
/// override if present, otherwise the field identifier. Excluded fields are not
/// valid targets.
fn validate_message_placeholders(def: &EventDef) -> Result<()> {
    let Some(message) = def.log.as_ref().and_then(|l| l.message.as_deref()) else {
        return Ok(());
    };

    // Quick scan: skip validation if there are no placeholders at all.
    if !message.contains('{') {
        return Ok(());
    }

    // Collect valid log key names.
    let log_key_names: Vec<String> = def.fields.iter().filter_map(FieldDef::log_key).collect();

    // Extract placeholders from the message template: substrings inside `{...}`.
    let mut rest = message;
    while let Some(open) = rest.find('{') {
        rest = &rest[open + 1..];
        // Escaped `{{` — skip.
        if rest.starts_with('{') {
            rest = &rest[1..];
            continue;
        }
        if let Some(close) = rest.find('}') {
            let placeholder = &rest[..close];
            if !placeholder.is_empty() && !log_key_names.iter().any(|name| name == placeholder) {
                return Err(Error::new_spanned(
                    &def.ident,
                    format!(
                        "log message references `{{{placeholder}}}` but no log attribute with that \
                     name exists; available attributes: [{}]",
                        log_key_names.join(", "),
                    ),
                ));
            }
            rest = &rest[close + 1..];
        }
    }

    Ok(())
}

/// Builds the `Option<LogDescription>` expression for the event's `#[log(...)]`
/// signal (or `None` when the event declares no log).
fn log_description_expr(def: &EventDef) -> TokenStream {
    let Some(log) = &def.log else {
        return quote! { ::core::option::Option::None };
    };
    let log_name = log.name.clone().unwrap_or_else(|| def.event_name.clone());
    let severity = log.severity;
    let body_expr = if let Some(b) = &log.message {
        quote! { ::core::option::Option::Some(#b) }
    } else {
        quote! { ::core::option::Option::None }
    };
    quote! {
        ::core::option::Option::Some(
            ::observed::metadata::LogDescription::new(
                #log_name,
                ::observed::Severity::#severity,
                #body_expr,
            )
        )
    }
}

fn generate_event_impl(def: &EventDef) -> TokenStream {
    let struct_ident = &def.ident;

    // Event identity comes from the required `#[event(name = "...")]` attribute.
    let event_name = &def.event_name;

    // Add 'static bound to type parameters so TypeId::of works.
    // Telemetry events already require Send + Sync; 'static is a practical requirement.
    let mut generics_with_static = def.generics.clone();
    for param in &mut generics_with_static.params {
        if let syn::GenericParam::Type(tp) = param {
            tp.bounds.push(syn::TypeParamBound::Lifetime(syn::Lifetime::new(
                "'static",
                proc_macro2::Span::call_site(),
            )));
        }
    }
    let (impl_generics, ty_generics, where_clause) = generics_with_static.split_for_impl();

    // Build type args with 'static substituted for lifetime params (lifetimes
    // are erased at runtime, so TypeId is the same regardless of actual lifetime).
    let static_type_args: Vec<_> = def
        .generics
        .params
        .iter()
        .map(|p| match p {
            syn::GenericParam::Lifetime(_) => quote! { 'static },
            syn::GenericParam::Type(tp) => {
                let ident = &tp.ident;
                quote! { #ident }
            }
            syn::GenericParam::Const(cp) => {
                let ident = &cp.ident;
                quote! { #ident }
            }
        })
        .collect();
    let type_id_args = if static_type_args.is_empty() {
        quote! {}
    } else {
        quote! { <#(#static_type_args),*> }
    };
    let type_id_expr = quote! {
        ::core::option::Option::Some(
            ::core::any::TypeId::of::<#struct_ident #type_id_args>()
        )
    };

    let log_expr = log_description_expr(def);

    let metric_expr = if let Some(metric) = &def.metric {
        let instrument_name = metric.name.clone().unwrap_or_else(|| def.event_name.clone());
        let description = metric.description.as_deref().unwrap_or("");
        let unit = metric.unit.as_deref().unwrap_or("");
        // A fieldless event-level metric is always a counter (records `1`).
        quote! {
            ::core::option::Option::Some(
                ::observed::metadata::MetricDescription::new(
                    #instrument_name,
                    ::observed::metadata::InstrumentKind::Counter,
                    #description,
                    #unit,
                )
            )
        }
    } else {
        quote! { ::core::option::Option::None }
    };

    let has_field_metrics = def.fields.iter().any(|f| f.metric_value.is_some());
    let disabled = def.disabled;
    let has_log = def.log.is_some();

    let visit_fields_body = generate_visit_fields_body(&def.fields, has_log);

    quote! {
        const _: () = {
            impl #impl_generics ::observed::Event for #struct_ident #ty_generics #where_clause {
                const DESCRIPTION: ::observed::metadata::EventDescription =
                    ::observed::metadata::EventDescription::new(
                        #event_name,
                        #type_id_expr,
                        #log_expr,
                        #metric_expr,
                        #has_field_metrics,
                        #disabled,
                    );

                fn visit_fields(
                    &self,
                    visitor: &mut ::observed::processing::FieldVisitorFn<'_>,
                ) -> ::core::ops::ControlFlow<()> {
                    use ::observed::Value;
                    use ::observed::metadata::{FieldDescriptor, LogFieldEntry, MetricFieldEntry};
                    #visit_fields_body
                    ::core::ops::ControlFlow::Continue(())
                }
            }
        };
    }
}

fn generate_visit_fields_body(fields: &[FieldDef], has_log: bool) -> TokenStream {
    let visits: Vec<TokenStream> = fields.iter().map(|f| generate_field_visit(f, has_log)).collect();
    quote! { #(#visits)* }
}

fn generate_field_visit(field: &FieldDef, has_log: bool) -> TokenStream {
    let field_ident = &field.ident;
    let default_key = field.ident.to_string();

    // Log routing
    let log_key = if has_log { field.log_key() } else { None };
    let log_entry = if let Some(key) = &log_key {
        quote! { ::core::option::Option::Some(LogFieldEntry::new(#key)) }
    } else {
        quote! { ::core::option::Option::None }
    };

    // Metric routing
    let metric_entry = if let Some(decl) = &field.metric_value {
        let name = &decl.name;
        let kind = decl.kind;
        let description = decl.description.as_deref().unwrap_or("");
        let unit = decl.unit.as_deref().unwrap_or("");
        quote! {
            ::core::option::Option::Some(MetricFieldEntry::instrument(
                #default_key,
                ::observed::metadata::MetricDescription::new(
                    #name,
                    ::observed::metadata::InstrumentKind::#kind,
                    #description,
                    #unit,
                ),
            ))
        }
    } else if let Some(key) = field.metric_dimension.resolve_key(&default_key) {
        quote! { ::core::option::Option::Some(MetricFieldEntry::dimension(#key)) }
    } else {
        quote! { ::core::option::Option::None }
    };

    let routed_to_log = log_key.is_some();

    // Skip emitting a visit if the field is not routed to any signal.
    let has_metric_routing = field.metric_value.is_some() || field.metric_dimension.is_dimension();
    if !routed_to_log && !has_metric_routing {
        return quote! {};
    }

    let field_desc = quote! {
        const FIELD_DESC: FieldDescriptor =
            FieldDescriptor::new(#default_key, #log_entry, #metric_entry);
    };

    // `Option<T>` fields dispatch on `#[if_none(...)]`: a `None` value is
    // either dropped or replaced with a placeholder string (default `"n/a"`).
    if let Some(inner_ty) = option_inner_type(&field.ty) {
        return generate_option_field_visit(field, inner_ty, &field_desc);
    }

    let owned = quote! { self.#field_ident.clone() };
    let by_ref = if is_reference_type(&field.ty) {
        // Field is already a reference (`&T`), so `self.field` is `&T`; pass it
        // directly to avoid a double-reference `&&T`.
        quote! { self.#field_ident }
    } else {
        // Field is an owned type, so `&self.field` produces `&T`.
        quote! { &self.#field_ident }
    };
    let value = value_expr(&field.redaction, &owned, &by_ref, &quote! { engine });
    let getter = if matches!(field.redaction, FieldRedaction::Unredacted) {
        quote! { |_| #value }
    } else {
        quote! { |engine| #value }
    };

    quote! {
        {
            #field_desc
            visitor(&FIELD_DESC, & #getter )?;
        }
    }
}

/// Builds the `Value::…` expression for a field value.
///
/// `owned` are tokens evaluating to an owned `T` (for the `Into<Value>` and
/// `Sensitive::new` paths); `by_ref` are tokens evaluating to `&T` (for the
/// redaction path); `engine` names the redaction engine bound in the enclosing
/// closure. The same helper drives the non-optional path and both arms of an
/// `Option<T>` field, so the three redaction variants are defined once.
fn value_expr(redaction: &FieldRedaction, owned: &TokenStream, by_ref: &TokenStream, engine: &TokenStream) -> TokenStream {
    match redaction {
        FieldRedaction::Unredacted => quote! { Value::from(#owned) },
        FieldRedaction::DataClass(expr) => quote! {
            Value::from_redacted(
                &::observed::__private::Sensitive::new(#owned, #expr),
                #engine)
        },
        FieldRedaction::Default => quote! { Value::from_redacted(#by_ref, #engine) },
    }
}

/// Generates the field-visit block for an `Option<T>` field.
///
/// - When the field is `Some(v)`, the inner value is captured exactly like a
///   non-optional field of type `T`.
/// - When the field is `None`, behavior follows `#[if_none(...)]`:
///   [`Drop`](IfNone::Drop) skips the field entirely (`visitor` is never
///   called), while [`Fill`](IfNone::Fill) records the placeholder string
///   in place of the missing value.
fn generate_option_field_visit(field: &FieldDef, inner_ty: &syn::Type, field_desc: &TokenStream) -> TokenStream {
    let field_ident = &field.ident;
    let inner_is_ref = is_reference_type(inner_ty);
    let engine = quote! { _engine };

    // `self.field.as_ref()` yields `Option<&inner>`, binding `__val: &inner`.
    // For `from_redacted` we need `&T`: when the inner type is already a reference
    // (`__val: &&T`) we deref once; otherwise `__val: &T` is used directly. The
    // owned form clones an owned inner, or copies the reference for a reference inner.
    let (val_owned, val_ref) = if inner_is_ref {
        (quote! { *__val }, quote! { *__val })
    } else {
        (quote! { __val.clone() }, quote! { __val })
    };
    let some_value = value_expr(&field.redaction, &val_owned, &val_ref, &engine);

    match &field.if_none {
        // `drop`: omit the field entirely when `None`.
        IfNone::Drop => quote! {
            if let ::core::option::Option::Some(__val) = self.#field_ident.as_ref() {
                #field_desc
                visitor(&FIELD_DESC, &|_engine| #some_value )?;
            }
        },
        // Fill: record the placeholder string in place of a missing value.
        IfNone::Fill(placeholder) => quote! {
            {
                #field_desc
                match self.#field_ident.as_ref() {
                    ::core::option::Option::Some(__val) => {
                        visitor(&FIELD_DESC, &|_engine| #some_value )?;
                    }
                    ::core::option::Option::None => {
                        visitor(&FIELD_DESC, &|_| Value::from(#placeholder))?;
                    }
                }
            }
        },
    }
}

// miri fails to use insta snapshots: `insta::_macro_support::get_cargo_workspace` leads to
#[cfg(all(test, not(miri)))]
mod tests {
    use super::*;

    fn parse_and_generate(input: &str) -> String {
        let input: DeriveInput = syn::parse_str(input).expect("failed to parse input");
        let tokens = derive_event(&input).expect("failed to derive");
        let file = syn::parse2(tokens).expect("failed to parse generated code");
        prettyplease::unparse(&file)
    }

    fn parse_and_expect_error(input: &str) -> String {
        let input: DeriveInput = syn::parse_str(input).expect("failed to parse input");
        derive_event(&input).expect_err("expected derive to fail").to_string()
    }

    #[test]
    fn test_basic_event() {
        let output = parse_and_generate(
            r#"
            #[event(name = "http.request")]
            #[log(severity = info)]
            struct HttpRequest {
                method: ClassifiedString,
                #[unredacted]
                status: i64,
            }
            "#,
        );
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_event_with_message() {
        let output = parse_and_generate(
            r#"
            #[event(name = "request.failed")]
            #[log(severity = warn, message = "Request failed")]
            struct RequestFailed {
                reason: ClassifiedString,
            }
            "#,
        );
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_event_with_field_attrs() {
        let output = parse_and_generate(
            r#"
            #[event(name = "my.event")]
            #[log(severity = info)]
            struct MyEvent {
                #[dimension(log = "custom_key")]
                request_id: ClassifiedString,
                #[unredacted]
                latency: f64,
            }
            "#,
        );
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_event_with_field_metric() {
        let output = parse_and_generate(
            r#"
            #[event(name = "outgoing_request")]
            #[log(severity = info, message = "Outgoing request")]
            #[metric(kind = histogram, field = duration, name = "request_duration", unit = "ms")]
            struct OutgoingRequest {
                method: ClassifiedString,
                request_id: ClassifiedString,
                operation: ClassifiedString,
                #[unredacted]
                duration: f64,
                #[dimension(log = exclude)]
                #[unredacted]
                internal_tag: i64,
            }
            "#,
        );
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_event_all_attributes() {
        let output = parse_and_generate(
            r#"
            #[event(name = "http.outgoing_request")]
            #[log(severity = error, message = "Outgoing HTTP request")]
            #[metric(kind = counter, name = "http.request.count")]
            #[metric(kind = histogram, field = duration, name = "request_duration")]
            struct FullEvent {
                #[dimension(log = "http.method")]
                method: ClassifiedString,
                #[dimension(metric = "op")]
                #[unredacted]
                operation: i64,
                #[unredacted]
                duration: f64,
            }
            "#,
        );
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_disabled_event() {
        let output = parse_and_generate(
            r#"
            #[event(name = "debug.diagnostics")]
            #[log(severity = debug, message = "Internal diagnostics")]
            #[metric(kind = gauge, field = queue_depth_metric, name = "debug.queue_depth")]
            #[disabled]
            struct DebugDiagnostics {
                #[unredacted]
                queue_depth: i64,
                #[unredacted]
                queue_depth_metric: f64,
            }
            "#,
        );
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_event_with_data_class() {
        let output = parse_and_generate(
            r#"
            #[event(name = "user.login")]
            #[log(severity = info)]
            struct UserLogin {
                #[data_class(DataTaxonomy::Euii)]
                username: String,
                #[unredacted]
                status: i64,
            }
            "#,
        );
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_counter_with_unsigned_field() {
        let output = parse_and_generate(
            r#"
            #[event(name = "bytes.received")]
            #[metric(kind = counter, field = bytes, name = "bytes.received.total", unit = "By")]
            struct BytesReceived {
                #[unredacted]
                bytes: u64,
            }
            "#,
        );
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_updowncounter_with_signed_field() {
        let output = parse_and_generate(
            r#"
            #[event(name = "queue.delta")]
            #[metric(kind = updown_counter, field = delta, name = "queue.size.delta")]
            struct QueueDelta {
                #[unredacted]
                delta: i64,
            }
            "#,
        );
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_event_metric_only() {
        // A fieldless counter records `1` per emission (event-level metric).
        let output = parse_and_generate(
            r#"
            #[event(name = "http.request.count")]
            #[metric(kind = counter)]
            struct CountEvent {
                #[unredacted]
                status: i64,
            }
            "#,
        );
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_event_field_metric_only() {
        let output = parse_and_generate(
            r#"
            #[event(name = "system.memory")]
            #[metric(kind = gauge, field = bytes, name = "system.memory.usage")]
            struct GaugeEvent {
                #[unredacted]
                bytes: i64,
            }
            "#,
        );
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_error_enum() {
        let err = parse_and_expect_error(
            r#"
            #[event(name = "bad")]
            #[log(severity = info)]
            enum BadEvent { A, B }
            "#,
        );
        assert!(err.contains("structs"), "{err}");
    }

    #[test]
    fn test_no_signal() {
        let output = parse_and_generate(
            r#"
            #[event(name = "no.signal")]
            struct NoSignal { x: String }
            "#,
        );
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_error_data_class_and_unredacted() {
        let err = parse_and_expect_error(
            r#"
            #[event(name = "bad")]
            #[log(severity = info)]
            struct BadEvent {
                #[data_class(Euii)]
                #[unredacted]
                x: String,
            }
            "#,
        );
        assert!(err.contains("mutually exclusive"), "{err}");
    }

    #[test]
    fn test_event_with_lifetime() {
        let output = parse_and_generate(
            r#"
            #[event(name = "borrowed.event")]
            #[log(severity = info)]
            struct BorrowedEvent<'a> {
                #[unredacted]
                message: &'a str,
            }
            "#,
        );
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_event_with_type_parameter() {
        let output = parse_and_generate(
            r#"
            #[event(name = "generic.event")]
            #[log(severity = info)]
            struct GenericEvent<T> {
                #[unredacted]
                value: T,
            }
            "#,
        );
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_event_with_where_clause() {
        let output = parse_and_generate(
            r#"
            #[event(name = "bounded.event")]
            #[log(severity = info)]
            struct BoundedEvent<T> where T: Clone {
                #[unredacted]
                value: T,
            }
            "#,
        );
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_error_duplicate_log_setting() {
        let err = parse_and_expect_error(
            r#"
            #[event(name = "bad")]
            #[log(severity = info)]
            struct BadEvent {
                #[dimension(log = "a", log = "b")]
                x: String,
            }
            "#,
        );
        assert!(err.contains("duplicate `log`"), "{err}");
    }

    #[test]
    fn test_error_duplicate_dimension() {
        let err = parse_and_expect_error(
            r#"
            #[event(name = "bad")]
            #[log(severity = info)]
            struct BadEvent {
                #[dimension(metric = "a")]
                #[dimension(metric = "b")]
                #[unredacted]
                x: i64,
            }
            "#,
        );
        assert!(err.contains("duplicate"), "{err}");
    }

    #[test]
    fn test_error_missing_event_name() {
        let err = parse_and_expect_error(
            r"
            #[log(severity = info)]
            struct MissingEventName {
                #[unredacted]
                x: i64,
            }
            ",
        );
        assert!(err.contains("event(name"), "{err}");
    }

    #[test]
    fn test_log_name_override() {
        let output = parse_and_generate(
            r#"
            #[event(name = "http.request")]
            #[log(severity = info, name = "http.request.log")]
            struct HttpRequest {
                #[unredacted]
                status: i64,
            }
            "#,
        );
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_error_duplicate_metric_setting() {
        let err = parse_and_expect_error(
            r#"
            #[event(name = "bad")]
            #[log(severity = info)]
            struct BadEvent {
                #[dimension(metric = "a", metric = "b")]
                x: String,
            }
            "#,
        );
        assert!(err.contains("duplicate `metric`"), "{err}");
    }

    #[test]
    fn test_error_duplicate_exclude_setting() {
        let err = parse_and_expect_error(
            r#"
            #[event(name = "bad")]
            #[log(severity = info)]
            struct BadEvent {
                #[dimension(log = exclude, log = exclude)]
                x: String,
            }
            "#,
        );
        assert!(err.contains("duplicate `log`"), "{err}");
    }

    #[test]
    fn test_error_unredacted_with_args() {
        let err = parse_and_expect_error(
            r#"
            #[event(name = "bad")]
            #[log(severity = info)]
            struct BadEvent {
                #[unredacted(foo)]
                x: String,
            }
            "#,
        );
        assert!(err.contains("does not accept arguments"), "{err}");
    }

    #[test]
    fn test_error_log_target_rejected() {
        // `#[log(target = "...")]` is no longer supported.
        let err = parse_and_expect_error(
            r#"
            #[event(name = "bad")]
            #[log(severity = info, target = "svc")]
            struct BadEvent {
                #[unredacted]
                x: i64,
            }
            "#,
        );
        assert!(err.contains("target"), "{err}");
    }

    #[test]
    fn test_error_message_references_nonexistent_attr() {
        let err = parse_and_expect_error(
            r#"
            #[event(name = "bad")]
            #[log(severity = info, message = "Hello {missing}")]
            struct BadEvent {
                #[unredacted]
                x: i64,
            }
            "#,
        );
        insta::assert_snapshot!(err);
    }

    #[test]
    fn test_error_message_references_field_name_not_attr_name() {
        let err = parse_and_expect_error(
            r#"
            #[event(name = "bad")]
            #[log(severity = info, message = "Value: {my_field}")]
            struct BadEvent {
                #[dimension(log = "custom_name")]
                #[unredacted]
                my_field: i64,
            }
            "#,
        );
        insta::assert_snapshot!(err);
    }

    #[test]
    fn test_message_references_renamed_attr_ok() {
        let _output = parse_and_generate(
            r#"
            #[event(name = "good")]
            #[log(severity = info, message = "Value: {custom_name}")]
            struct GoodEvent {
                #[dimension(log = "custom_name")]
                #[unredacted]
                my_field: i64,
            }
            "#,
        );
    }

    #[test]
    fn severity_accepts_string_literal_form() {
        // The string-literal arm of `ident_or_str` must keep parsing
        // `severity = "info"` (not just the bare-ident form).
        let _output = parse_and_generate(
            r#"
            #[event(name = "e")]
            #[log(severity = "info")]
            struct E {
                #[unredacted]
                x: i64,
            }
            "#,
        );
    }

    #[test]
    fn severity_rejects_multi_segment_path() {
        // A multi-segment path has no single ident, so the guarded ident arm
        // must not match (mutating the guard to always-match would panic).
        let err = parse_and_expect_error(
            r#"
            #[event(name = "e")]
            #[log(severity = foo::bar)]
            struct E {
                #[unredacted]
                x: i64,
            }
            "#,
        );
        assert!(!err.is_empty());
    }

    #[test]
    fn metric_error_names_the_instrument_kind() {
        // A fieldless non-counter metric is rejected, and the message must
        // spell out the offending kind via `InstrumentKindValue::attr_name`.
        let err = parse_and_expect_error(
            r#"
            #[event(name = "e")]
            #[metric(kind = gauge)]
            struct E {
                #[unredacted]
                x: i64,
            }
            "#,
        );
        assert!(err.contains("gauge"), "error should name the kind: {err}");
    }

    #[test]
    fn message_with_unknown_placeholder_is_rejected() {
        // Guards the `{`-offset arithmetic in `validate_message_placeholders`:
        // the placeholder must be extracted exactly so an unknown one errors.
        let err = parse_and_expect_error(
            r#"
            #[event(name = "e")]
            #[log(severity = info, message = "Value: {nonexistent}")]
            struct E {
                #[unredacted]
                x: i64,
            }
            "#,
        );
        assert!(err.contains("`{nonexistent}`"), "{err}");
    }

    #[test]
    fn test_error_message_references_excluded_field() {
        let err = parse_and_expect_error(
            r#"
            #[event(name = "bad")]
            #[log(severity = info, message = "Tag: {tag}")]
            struct BadEvent {
                #[dimension(log = exclude)]
                #[unredacted]
                tag: i64,
            }
            "#,
        );
        insta::assert_snapshot!(err);
    }

    #[test]
    fn test_unit_struct_event() {
        let output = parse_and_generate(
            r#"
            #[event(name = "workload.disabled")]
            #[log(severity = info)]
            struct NoV2Workloads;
            "#,
        );
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_event_with_reference_to_redactable_type() {
        let output = parse_and_generate(
            r#"
            #[event(name = "borrowed.classified")]
            #[log(severity = info)]
            struct BorrowedClassified<'a> {
                name: &'a PiiString,
                #[unredacted]
                count: i64,
            }
            "#,
        );
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_event_with_parenthesized_reference() {
        let output = parse_and_generate(
            r#"
            #[event(name = "paren.ref")]
            #[log(severity = info)]
            struct ParenRef<'a> {
                name: (&'a PiiString),
            }
            "#,
        );
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_option_field_filled_when_none() {
        // By default a `None` `Option<T>` is filled with the `"N/A"` placeholder.
        let output = parse_and_generate(
            r#"
            #[event(name = "http.request")]
            #[log(severity = info)]
            struct HttpRequest {
                method: ClassifiedString,
                user_agent: Option<ClassifiedString>,
            }
            "#,
        );
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_option_field_drop_when_none() {
        // `#[if_none(drop)]` omits the field entirely when `None`.
        let output = parse_and_generate(
            r#"
            #[event(name = "http.request")]
            #[log(severity = info)]
            struct HttpRequest {
                method: ClassifiedString,
                #[if_none(drop)]
                user_agent: Option<ClassifiedString>,
            }
            "#,
        );
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_option_field_custom_fill_when_none() {
        // `#[if_none("...")]` records a custom placeholder.
        let output = parse_and_generate(
            r#"
            #[event(name = "http.request")]
            #[log(severity = info)]
            struct HttpRequest {
                method: ClassifiedString,
                #[if_none("unknown")]
                user_agent: Option<ClassifiedString>,
            }
            "#,
        );
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_option_dimension_filled_when_none() {
        // `Option<T>` metric dimension without a value: filled with `"n/a"` when `None`.
        let output = parse_and_generate(
            r#"
            #[event(name = "http.request.count")]
            #[metric(kind = counter)]
            struct CountEvent {
                #[dimension(metric = "region")]
                #[unredacted]
                region: Option<String>,
            }
            "#,
        );
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_error_if_none_on_non_option() {
        let err = parse_and_expect_error(
            r#"
            #[event(name = "bad")]
            #[log(severity = info)]
            struct BadEvent {
                #[if_none(drop)]
                #[unredacted]
                count: i64,
            }
            "#,
        );
        assert!(err.contains("only valid on `Option<T>`"), "{err}");
    }

    #[test]
    fn test_error_counter_signed_field() {
        let err = parse_and_expect_error(
            r#"
            #[event(name = "bad")]
            #[metric(kind = counter, field = count)]
            struct BadEvent {
                #[unredacted]
                count: i64,
            }
            "#,
        );
        assert!(err.contains("unsigned integer"), "{err}");
    }

    #[test]
    fn test_error_updowncounter_unsigned_field() {
        let err = parse_and_expect_error(
            r#"
            #[event(name = "bad")]
            #[metric(kind = updown_counter, field = delta)]
            struct BadEvent {
                #[unredacted]
                delta: u64,
            }
            "#,
        );
        assert!(err.contains("signed integer"), "{err}");
    }

    #[test]
    fn test_error_counter_non_integer_field() {
        let err = parse_and_expect_error(
            r#"
            #[event(name = "bad")]
            #[metric(kind = counter, field = count)]
            struct BadEvent {
                #[unredacted]
                count: f64,
            }
            "#,
        );
        assert!(err.contains("unsigned integer"), "{err}");
    }

    #[test]
    fn test_error_metric_field_not_found() {
        let err = parse_and_expect_error(
            r#"
            #[event(name = "bad")]
            #[metric(kind = histogram, field = nope)]
            struct BadEvent {
                #[unredacted]
                duration: f64,
            }
            "#,
        );
        assert!(err.contains("does not exist"), "{err}");
    }

    #[test]
    fn test_error_gauge_requires_field() {
        let err = parse_and_expect_error(
            r#"
            #[event(name = "bad")]
            #[metric(kind = gauge, name = "x")]
            struct BadEvent {
                #[unredacted]
                value: f64,
            }
            "#,
        );
        assert!(err.contains("requires `field"), "{err}");
    }

    #[test]
    fn test_error_updowncounter_requires_field() {
        let err = parse_and_expect_error(
            r#"
            #[event(name = "bad")]
            #[metric(kind = updown_counter, name = "x")]
            struct BadEvent {
                #[unredacted]
                value: i64,
            }
            "#,
        );
        assert!(err.contains("requires `field"), "{err}");
    }

    #[test]
    fn test_error_instrument_on_field() {
        let err = parse_and_expect_error(
            r#"
            #[event(name = "bad")]
            struct BadEvent {
                #[metric(kind = counter, field = x)]
                #[unredacted]
                x: u64,
            }
            "#,
        );
        assert!(err.contains("struct-level attribute"), "{err}");
    }

    #[test]
    fn test_error_metric_missing_kind() {
        // `#[metric(...)]` requires a `kind = ...` argument.
        let err = parse_and_expect_error(
            r#"
            #[event(name = "bad")]
            #[metric(field = duration)]
            struct BadEvent {
                #[unredacted]
                duration: f64,
            }
            "#,
        );
        assert!(err.contains("kind"), "{err}");
    }

    #[test]
    fn test_error_metric_unknown_kind() {
        // An unrecognized `kind` value is rejected.
        let err = parse_and_expect_error(
            r#"
            #[event(name = "bad")]
            #[metric(kind = Summary, field = duration)]
            struct BadEvent {
                #[unredacted]
                duration: f64,
            }
            "#,
        );
        assert!(err.contains("counter"), "{err}");
    }

    #[test]
    fn test_error_field_both_metric_and_dimension() {
        let err = parse_and_expect_error(
            r#"
            #[event(name = "bad")]
            #[metric(kind = histogram, field = duration)]
            struct BadEvent {
                #[dimension(metric = "duration")]
                #[unredacted]
                duration: f64,
            }
            "#,
        );
        assert!(err.contains("cannot be both"), "{err}");
    }

    #[test]
    fn test_error_duplicate_metric_on_field() {
        let err = parse_and_expect_error(
            r#"
            #[event(name = "bad")]
            #[metric(kind = histogram, field = duration)]
            #[metric(kind = gauge, field = duration)]
            struct BadEvent {
                #[unredacted]
                duration: f64,
            }
            "#,
        );
        assert!(err.contains("already has a metric"), "{err}");
    }

    #[test]
    fn test_error_duplicate_event_metric() {
        let err = parse_and_expect_error(
            r#"
            #[event(name = "bad")]
            #[metric(kind = counter)]
            #[metric(kind = counter, name = "other")]
            struct BadEvent {
                #[unredacted]
                x: i64,
            }
            "#,
        );
        assert!(err.contains("only one event-level"), "{err}");
    }

    #[test]
    fn test_error_dimension_positional_rejected() {
        // The positional string form `#[dimension("region")]` is no longer
        // supported; users must write `#[dimension(log = "region")]`.
        let err = parse_and_expect_error(
            r#"
            #[event(name = "http.request.count")]
            #[log(severity = info)]
            #[metric(kind = counter)]
            struct CountEvent {
                #[dimension("region")]
                #[unredacted]
                region: i64,
            }
            "#,
        );
        assert!(err.contains("positional string name is not supported"), "{err}");
    }

    #[test]
    fn test_dimension_bare_logs_under_own_name() {
        // A bare `#[dimension]` logs the field under its own name and opts out of
        // the metric signal - equivalent to the field's default routing.
        let output = parse_and_generate(
            r#"
            #[event(name = "http.request")]
            #[log(severity = info)]
            struct HttpRequest {
                #[dimension]
                #[unredacted]
                internal_flag: i64,
            }
            "#,
        );
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_dimension_metric_bare_opts_in_own_name() {
        // Bare `metric` opts the field in as a metric dimension keyed by the field
        // name, while it remains logged under its own name.
        let output = parse_and_generate(
            r#"
            #[event(name = "http.request.count")]
            #[log(severity = info)]
            #[metric(kind = counter)]
            struct CountEvent {
                #[dimension(metric)]
                #[unredacted]
                region: i64,
            }
            "#,
        );
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_dimension_exclude_log_with_bare_metric() {
        // `log = exclude, metric` drops the field from logs while opting it in as a
        // metric dimension keyed by the field name.
        let output = parse_and_generate(
            r#"
            #[event(name = "http.request.count")]
            #[log(severity = info)]
            #[metric(kind = counter)]
            struct CountEvent {
                #[dimension(log = exclude, metric)]
                #[unredacted]
                region: i64,
            }
            "#,
        );
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_dimension_separate_log_and_metric_keys() {
        // `log` and `metric` can name the two signals independently.
        let output = parse_and_generate(
            r#"
            #[event(name = "http.request.count")]
            #[log(severity = info)]
            #[metric(kind = counter)]
            struct CountEvent {
                #[dimension(log = "http.status_code", metric = "status")]
                #[unredacted]
                status: i64,
            }
            "#,
        );
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_dimension_metric_only_excludes_log() {
        // `log = exclude` removes the field from the log while `metric` keeps it
        // as a metric dimension.
        let output = parse_and_generate(
            r#"
            #[event(name = "http.request.count")]
            #[log(severity = info)]
            #[metric(kind = counter)]
            struct CountEvent {
                #[dimension(log = exclude, metric = "region")]
                #[unredacted]
                region: i64,
            }
            "#,
        );
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_dimension_metric_keeps_default_log() {
        // With only `metric` set, the field is still logged under its own name.
        let output = parse_and_generate(
            r#"
            #[event(name = "http.request.count")]
            #[log(severity = info)]
            #[metric(kind = counter)]
            struct CountEvent {
                #[dimension(metric = "region")]
                #[unredacted]
                region: i64,
            }
            "#,
        );
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_error_dimension_bare_exclude() {
        // The bare `#[dimension(exclude)]` shorthand is not supported; use
        // `#[dimension(log = exclude)]` instead.
        let err = parse_and_expect_error(
            r#"
            #[event(name = "http.request")]
            #[log(severity = info)]
            struct HttpRequest {
                #[dimension(exclude)]
                #[unredacted]
                internal_flag: i64,
            }
            "#,
        );
        assert!(err.contains("log = exclude"), "{err}");
    }

    #[test]
    fn test_metric_value_field_can_exclude_from_log() {
        // A metric value field may still control its own log routing: `log =
        // exclude` removes the value from the log without making it a dimension.
        let output = parse_and_generate(
            r#"
            #[event(name = "outgoing_request")]
            #[log(severity = info)]
            #[metric(kind = histogram, field = duration)]
            struct OutgoingRequest {
                method: ClassifiedString,
                #[dimension(log = exclude)]
                #[unredacted]
                duration: f64,
            }
            "#,
        );
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_error_empty_dimension() {
        let err = parse_and_expect_error(
            r#"
            #[event(name = "bad")]
            #[log(severity = info)]
            struct BadEvent {
                #[dimension()]
                #[unredacted]
                x: i64,
            }
            "#,
        );
        assert!(err.contains("requires at least one"), "{err}");
    }

    #[test]
    fn test_error_dimension_log_specified_twice() {
        // Two `log` items in one `#[dimension(...)]` is a duplicate-`log` error.
        let err = parse_and_expect_error(
            r#"
            #[event(name = "bad")]
            #[log(severity = info)]
            struct BadEvent {
                #[dimension(log = "a", log = "b")]
                #[unredacted]
                x: i64,
            }
            "#,
        );
        assert!(err.contains("duplicate `log`"), "{err}");
    }

    #[test]
    fn test_error_dimension_bad_log_value() {
        let err = parse_and_expect_error(
            r#"
            #[event(name = "bad")]
            #[log(severity = info)]
            struct BadEvent {
                #[dimension(log = nope)]
                #[unredacted]
                x: i64,
            }
            "#,
        );
        assert!(err.contains("string key or the `exclude`"), "{err}");
    }
}
