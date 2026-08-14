// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Implementation of the `#[event(...)]` attribute macro.
//!
//! Two phases:
//! 1. **Parse phase**: parse the `#[event(...)]` arguments and the annotated
//!    `ItemStruct` (with its sibling log/metric helper attributes) into
//!    intermediate structs.
//! 2. **Code generation phase**: generate a `TokenStream` (the re-emitted struct
//!    plus its `Event` impl) from the parsed definitions.
//!
//! See the [`event`](macro@crate::event) attribute macro documentation for the full
//! attribute syntax reference.

use proc_macro2::TokenStream;
use quote::{ToTokens, format_ident, quote};
use syn::ext::IdentExt as _;
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{Attribute, Error, Field, Fields, Generics, Ident, ItemStruct, LitStr, Meta, Result, Token};

use crate::field_attrs::{
    Dimension, FieldRedaction, IfNone, LogRouting, MetricRouting, SharedFieldAttrs, is_borrowed_str, is_reference_type, option_inner_type,
};

// ================================================================================================
// Attribute argument structs
// ================================================================================================

/// Parsed definition of an event struct.
struct EventDef {
    ident: Ident,
    /// The canonical event name from `#[event("...")]`.
    event_name: String,
    generics: Generics,
    log: Option<LogArgs>,
    /// Event-level metric (a fieldless `#[counter]`), which records `1` per
    /// emission. Always a counter instrument.
    metric: Option<EventMetric>,
    disabled: bool,
    fields: Vec<FieldDef>,
}

/// Arguments to the `#[event("name" [, disabled])]` attribute macro.
struct EventArgs {
    name: String,
    disabled: bool,
}

impl Parse for EventArgs {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let name: LitStr = input.parse().map_err(|err| {
            Error::new(
                err.span(),
                "`#[event(...)]` requires a string event name, e.g. `#[event(\"http.request\")]`",
            )
        })?;
        let mut disabled = false;
        while input.peek(Token![,]) {
            input.parse::<Token![,]>()?;
            if input.is_empty() {
                break;
            }
            let flag: Ident = input.parse()?;
            if flag == "disabled" {
                disabled = true;
            } else {
                return Err(Error::new_spanned(
                    &flag,
                    format!("unknown `#[event(...)]` flag `{flag}`; expected `disabled`"),
                ));
            }
        }
        Ok(Self {
            name: name.value(),
            disabled,
        })
    }
}

/// Resolved `#[<severity>(...)]` log declaration. The severity comes from the
/// attribute name; the optional positional string is the log body/message.
struct LogArgs {
    severity: SeverityKind,
    name: Option<String>,
    message: Option<String>,
}

/// Parsed body of a `#[<severity>("message" [, name = "..."])]` attribute.
struct LogAttrBody {
    message: Option<String>,
    name: Option<String>,
}

impl Parse for LogAttrBody {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let mut message = None;
        if input.peek(LitStr) {
            message = Some(input.parse::<LitStr>()?.value());
            if !input.is_empty() {
                input.parse::<Token![,]>()?;
            }
        }
        let mut name = None;
        let named = Punctuated::<NamedStr, Token![,]>::parse_terminated(input)?;
        for NamedStr { key, value } in named {
            if key == "name" {
                set_str_once(&mut name, value, &key, "name")?;
            } else {
                return Err(Error::new_spanned(&key, format!("unknown log option `{key}`; expected `name`")));
            }
        }
        Ok(Self { message, name })
    }
}

/// Resolved struct-level instrument declaration.
///
/// `kind` selects the instrument type (`counter`, `updown_counter`, `gauge`, or
/// `histogram`) and is taken from the attribute name (`#[gauge(...)]`).
///
/// `field` names the struct field whose value the instrument records, written as
/// the leading positional identifier (`#[histogram(duration_ms, ...)]`). It is
/// optional for `counter` (a fieldless counter records `1` per emission) and
/// required for every other instrument kind.
///
/// `name` overrides the instrument's metric name (defaults to the event name).
/// `desc` and `unit` supply the corresponding OpenTelemetry metadata.
struct InstrumentArgs {
    kind: InstrumentKindValue,
    field: Option<Ident>,
    name: Option<String>,
    description: Option<String>,
    unit: Option<String>,
}

/// Parsed body of a `#[<kind>([field] [, name = ...] [, desc = ...] [, unit = ...])]`
/// metric attribute (the instrument `kind` comes from the attribute name).
struct MetricAttrBody {
    field: Option<Ident>,
    name: Option<String>,
    description: Option<String>,
    unit: Option<String>,
}

impl Parse for MetricAttrBody {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        // A leading bare identifier (not `ident = ...`) is the positional field.
        let mut field = None;
        if input.peek(Ident) && !input.peek2(Token![=]) {
            field = Some(input.parse()?);
            if !input.is_empty() {
                input.parse::<Token![,]>()?;
            }
        }
        let mut name = None;
        let mut description = None;
        let mut unit = None;
        let named = Punctuated::<NamedStr, Token![,]>::parse_terminated(input)?;
        for NamedStr { key, value } in named {
            match key.to_string().as_str() {
                "name" => set_str_once(&mut name, value, &key, "name")?,
                "desc" => set_str_once(&mut description, value, &key, "desc")?,
                "unit" => set_str_once(&mut unit, value, &key, "unit")?,
                other => {
                    return Err(Error::new_spanned(
                        &key,
                        format!("unknown metric option `{other}`; expected `name`, `desc`, or `unit`"),
                    ));
                }
            }
        }
        Ok(Self {
            field,
            name,
            description,
            unit,
        })
    }
}

/// A `key = "value"` pair used inside log and metric attribute bodies.
struct NamedStr {
    key: Ident,
    value: String,
}

impl Parse for NamedStr {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let key: Ident = input.parse()?;
        input.parse::<Token![=]>()?;
        let value: LitStr = input.parse()?;
        Ok(Self { key, value: value.value() })
    }
}

/// Assigns `value` to `slot` if unset, otherwise reports a duplicate-`what` error.
fn set_str_once(slot: &mut Option<String>, value: String, key: &Ident, what: &str) -> Result<()> {
    if slot.is_some() {
        return Err(Error::new_spanned(key, format!("duplicate `{what}` setting")));
    }
    *slot = Some(value);
    Ok(())
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
    ///
    /// The name is unraw-ed, so a field written as `r#type` is logged under the
    /// domain key `type` rather than leaking Rust's raw-identifier escape into
    /// telemetry.
    fn log_key(&self) -> Option<String> {
        match &self.log {
            LogRouting::Default => Some(self.ident.unraw().to_string()),
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

impl SeverityKind {
    /// Maps a log-severity attribute name (`#[info]`, `#[warning]`, ...) to a
    /// severity, or `None` if the identifier is not a known severity.
    ///
    /// The `Warn` severity is spelled `warning` (not `warn`) because `warn` is a
    /// built-in lint attribute that cannot be used as a custom attribute.
    fn from_ident(ident: &Ident) -> Option<Self> {
        Some(match ident.to_string().as_str() {
            "trace" => Self::Trace,
            "debug" => Self::Debug,
            "info" => Self::Info,
            "warning" => Self::Warn,
            "error" => Self::Error,
            "fatal" => Self::Fatal,
            _ => return None,
        })
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

impl InstrumentKindValue {
    /// Maps a metric-kind attribute name (`#[counter]`, `#[gauge]`, ...) to an
    /// instrument kind, or `None` if the identifier is not a known kind.
    fn from_ident(ident: &Ident) -> Option<Self> {
        Some(match ident.to_string().as_str() {
            "counter" => Self::Counter,
            "updown_counter" => Self::UpDownCounter,
            "gauge" => Self::Gauge,
            "histogram" => Self::Histogram,
            _ => return None,
        })
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

/// How a primitive numeric type may be used as a metric instrument's value.
///
/// Only types [`observed::Value`] can carry are listed. `u128`/`i128` are
/// deliberately absent: no telemetry backend represents them, so `Value` offers
/// no conversion and the macro rejects them with a dedicated diagnostic rather
/// than truncating.
#[derive(Clone, Copy, PartialEq, Eq)]
enum NumericKind {
    /// A signed integer: `i8`, `i16`, `i32`, `i64`, `isize`.
    SignedInt,
    /// An unsigned integer: `u8`, `u16`, `u32`, `u64`, `usize`.
    UnsignedInt,
    /// A float: `f32`, `f64`.
    Float,
}

/// Classifies a primitive numeric type, matched syntactically on the last path
/// segment (so `u64` and `std::primitive::u64` are recognized, but a type
/// aliased to an integer is not). Returns `None` for non-numeric types,
/// unrecognized paths, and the unsupported 128-bit widths. Group and
/// parenthesis wrappers are transparent; `Option<T>` deliberately is **not**, so
/// an optional field can never satisfy an instrument's value-type requirement.
fn numeric_kind(ty: &syn::Type) -> Option<NumericKind> {
    let syn::Type::Path(type_path) = strip_type_wrappers(ty) else {
        return None;
    };
    let ident = type_path.path.segments.last()?.ident.to_string();
    match ident.as_str() {
        "u8" | "u16" | "u32" | "u64" | "usize" => Some(NumericKind::UnsignedInt),
        "i8" | "i16" | "i32" | "i64" | "isize" => Some(NumericKind::SignedInt),
        "f32" | "f64" => Some(NumericKind::Float),
        _ => None,
    }
}

/// Returns true for the 128-bit integer widths, which are recognized only so
/// they can be rejected with a specific diagnostic instead of the generic
/// "not a supported numeric type" one.
fn is_128_bit_int(ty: &syn::Type) -> bool {
    let syn::Type::Path(type_path) = strip_type_wrappers(ty) else {
        return false;
    };
    type_path
        .path
        .segments
        .last()
        .is_some_and(|s| matches!(s.ident.to_string().as_str(), "u128" | "i128"))
}

/// Strips transparent `Paren`/`Group` wrappers from a type.
fn strip_type_wrappers(ty: &syn::Type) -> &syn::Type {
    match ty {
        syn::Type::Paren(inner) => strip_type_wrappers(&inner.elem),
        syn::Type::Group(inner) => strip_type_wrappers(&inner.elem),
        other => other,
    }
}

// ================================================================================================
// Parse phase
// ================================================================================================

fn parse_event_def(ident: &Ident, generics: &Generics, attrs: &[Attribute], fields_data: &Fields, args: &EventArgs) -> Result<EventDef> {
    let fields: Vec<&Field> = match fields_data {
        Fields::Named(fields) => fields.named.iter().collect(),
        Fields::Unit => Vec::new(),
        Fields::Unnamed(_) => {
            return Err(Error::new_spanned(
                ident,
                "#[event] can only be applied to structs with named fields",
            ));
        }
    };

    let mut log: Option<LogArgs> = None;
    let mut metric_specs: Vec<MetricSpec> = Vec::new();

    for attr in attrs {
        let Some(ident) = attr.path().get_ident() else {
            continue;
        };
        if let Some(severity) = SeverityKind::from_ident(ident) {
            if log.is_some() {
                return Err(Error::new_spanned(
                    attr,
                    "only one log-severity attribute (`#[trace]`, `#[debug]`, `#[info]`, \
                     `#[warning]`, `#[error]`, `#[fatal]`) is allowed",
                ));
            }
            let body = parse_log_attr(attr)?;
            log = Some(LogArgs {
                severity,
                name: body.name,
                message: body.message,
            });
        } else if let Some(kind) = InstrumentKindValue::from_ident(ident) {
            let body = parse_metric_attr(attr)?;
            metric_specs.push(MetricSpec {
                args: InstrumentArgs {
                    kind,
                    field: body.field,
                    name: body.name,
                    description: body.description,
                    unit: body.unit,
                },
                attr: attr.clone(),
            });
        }
    }

    let mut field_defs = Vec::with_capacity(fields.len());
    for field in fields {
        field_defs.push(parse_field_def(field)?);
    }

    let event_name = args.name.clone();
    let metric = resolve_metrics(metric_specs, &mut field_defs, &event_name)?;

    Ok(EventDef {
        ident: ident.clone(),
        event_name,
        generics: generics.clone(),
        log,
        metric,
        disabled: args.disabled,
        fields: field_defs,
    })
}

/// Parses the body of a log-severity attribute (`#[info]`, `#[warning("body")]`, ...).
fn parse_log_attr(attr: &Attribute) -> Result<LogAttrBody> {
    match &attr.meta {
        Meta::Path(_) => Ok(LogAttrBody { message: None, name: None }),
        Meta::List(_) => attr.parse_args::<LogAttrBody>(),
        Meta::NameValue(_) => Err(Error::new_spanned(
            attr,
            "a log-severity attribute takes an optional message string and `name = \"...\"`, \
             not `= value`",
        )),
    }
}

/// Parses the body of a metric-kind attribute (`#[counter(...)]`, `#[gauge(f, ...)]`, ...).
fn parse_metric_attr(attr: &Attribute) -> Result<MetricAttrBody> {
    match &attr.meta {
        Meta::Path(_) => Ok(MetricAttrBody {
            field: None,
            name: None,
            description: None,
            unit: None,
        }),
        Meta::List(_) => attr.parse_args::<MetricAttrBody>(),
        Meta::NameValue(_) => Err(Error::new_spanned(
            attr,
            "a metric attribute takes an optional field and `name`/`desc`/`unit` options, \
             not `= value`",
        )),
    }
}

/// Resolves struct-level instrument declarations against the event's fields.
///
/// Each `#[<kind>(<field>, ...)]` attaches its instrument to the
/// named field (whose value becomes the measurement). A fieldless
/// `#[counter]` becomes the event-level metric that records `1`
/// per emission. In both cases the instrument's metric name defaults to the
/// event name; an explicit `name = "..."` overrides it. Returns the
/// event-level metric, if any.
fn resolve_metrics(specs: Vec<MetricSpec>, fields: &mut [FieldDef], event_name: &str) -> Result<Option<EventMetric>> {
    let mut event_metric: Option<EventMetric> = None;

    for spec in specs {
        let MetricSpec { args, attr } = spec;
        let kind = args.kind;

        let Some(field_ident) = args.field.clone() else {
            // Fieldless instrument: only `counter` is allowed, and it
            // records `1` per emission as the event-level metric.
            if !matches!(kind, InstrumentKindValue::Counter) {
                return Err(Error::new_spanned(
                    &attr,
                    format!(
                        "`#[{}(...)]` requires a field naming the struct field \
                         that holds the metric value, e.g. `#[{}(duration_ms)]`",
                        kind.attr_name(),
                        kind.attr_name(),
                    ),
                ));
            }
            if event_metric.is_some() {
                return Err(Error::new_spanned(
                    &attr,
                    "only one event-level metric (a fieldless `#[counter]`) is allowed",
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
                    "`#[{}({field_ident})]` references field \
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
        // An instrument records a measurement on every emission, so its value field must always
        // hold one. For an `Option<T>` the `#[if_none(...)]` default fills `None` with a
        // placeholder *string*, which is not a valid measurement for a numeric instrument -- the
        // downstream processor would then either drop the point or fail on the type mismatch.
        if option_inner_type(&field.ty).is_some() {
            return Err(Error::new_spanned(
                &attr,
                format!(
                    "`#[{}({field_ident})]` requires field `{field_ident}` to hold a value on \
                     every emission, but it is an `Option<T>`; a metric value cannot be optional. \
                     Use a non-optional field, or record it as a metric dimension instead \
                     (`#[dimension(metric = ...)]`)",
                    kind.attr_name(),
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

/// Enforces the metric value contract.
///
/// A metric instrument records a measurement on every emission, so its value
/// field has to arrive as a number. Two things can stop that, and both used to
/// pass validation and then drop the measurement at runtime:
///
/// - **Redaction.** Anything that is not `#[unredacted]` is rendered through
///   `Value::from_redacted`, which always produces a string. A metric processor
///   cannot take a measurement from a string.
/// - **The value type.** `Value` carries `i8`-`i64`/`isize`, `u8`-`u64`/`usize`
///   and `f32`/`f64`. Anything else - a string newtype, a bool, `u128`/`i128` -
///   has no numeric representation to record.
///
/// On top of that, `counter` requires an unsigned integer and `updown_counter`
/// a signed one; `gauge` and `histogram` accept any supported numeric type,
/// floats included. `Option<T>` never satisfies any of these, but it is
/// rejected earlier with a dedicated diagnostic.
fn enforce_value_type(kind: InstrumentKindValue, field: &FieldDef, attr: &syn::Attribute) -> Result<()> {
    let field_ident = &field.ident;
    let attr_name = kind.attr_name();

    if !matches!(field.redaction, FieldRedaction::Unredacted) {
        return Err(Error::new_spanned(
            attr,
            format!(
                "`#[{attr_name}({field_ident})]` requires field `{field_ident}` to be `#[unredacted]`; a classified value is \
                 rendered through the redaction engine as a string, which carries no measurement for the instrument to record",
            ),
        ));
    }

    if is_128_bit_int(&field.ty) {
        return Err(Error::new_spanned(
            attr,
            format!(
                "`#[{attr_name}({field_ident})]` does not support 128-bit integers; no telemetry backend represents them, so \
                 `observed::Value` has no conversion. Use a 64-bit width instead",
            ),
        ));
    }

    let Some(actual) = numeric_kind(&field.ty) else {
        return Err(Error::new_spanned(
            attr,
            format!(
                "`#[{attr_name}({field_ident})]` requires field `{field_ident}` to be a numeric type that \
                 `observed::Value` can carry (i8, i16, i32, i64, isize, u8, u16, u32, u64, usize, f32, f64)",
            ),
        ));
    };

    // Counter and up-down counter narrow the accepted set by signedness;
    // gauge and histogram accept every supported numeric, so the checks above
    // are their whole contract. Pairing each kind with its diagnostic wording
    // here keeps the match exhaustive without a fourth, unreachable arm.
    let (required, word, examples) = match kind {
        InstrumentKindValue::Counter => (NumericKind::UnsignedInt, "unsigned", "u8, u16, u32, u64, usize"),
        InstrumentKindValue::UpDownCounter => (NumericKind::SignedInt, "signed", "i8, i16, i32, i64, isize"),
        InstrumentKindValue::Gauge | InstrumentKindValue::Histogram => return Ok(()),
    };

    if actual == required {
        return Ok(());
    }

    Err(Error::new_spanned(
        attr,
        format!("`#[{attr_name}(...)]` requires field `{field_ident}` to be a {word} integer type ({examples})"),
    ))
}

fn parse_field_def(field: &Field) -> Result<FieldDef> {
    let ident = field.ident.clone().expect("named fields should have identifiers");

    let mut shared = SharedFieldAttrs::default();

    for attr in &field.attrs {
        if shared.try_parse(attr)? {
            continue;
        }
        if attr
            .path()
            .get_ident()
            .is_some_and(|i| InstrumentKindValue::from_ident(i).is_some())
        {
            return Err(Error::new_spanned(
                attr,
                "a metric instrument is a struct-level attribute; place it on the event struct \
                 and name this field positionally, e.g. `#[histogram(duration_ms)]`",
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

/// Entry point for `#[event("name" [, disabled])]`.
///
/// Consumes the struct's log/metric/field helper attributes, strips them from
/// the re-emitted struct, and appends the generated `Event` impl. Because the
/// attribute macro expands before its sibling attributes are resolved, the
/// helper attributes never need to resolve as real macros. (The `warn` severity
/// is spelled `#[warning(...)]` to avoid the built-in `warn` lint attribute,
/// which rustc validates before macro expansion.)
pub(crate) fn event_attr(attr: TokenStream, item: TokenStream) -> Result<TokenStream> {
    let args: EventArgs = syn::parse2(attr)?;
    let item_struct: ItemStruct = syn::parse2(item)?;
    let impl_tokens = generate_event(
        &item_struct.ident,
        &item_struct.generics,
        &item_struct.attrs,
        &item_struct.fields,
        &args,
    )?;
    let cleaned = strip_helper_attrs(item_struct);
    Ok(quote! {
        #cleaned
        #impl_tokens
    })
}

/// Generates just the `Event` trait impl (without re-emitting the struct).
/// Shared by the [`event_attr`] entry point and the unit tests.
fn generate_event(ident: &Ident, generics: &Generics, attrs: &[Attribute], fields: &Fields, args: &EventArgs) -> Result<TokenStream> {
    let def = parse_event_def(ident, generics, attrs, fields, args)?;
    validate_message_placeholders(&def)?;
    Ok(generate_event_impl(&def))
}

/// Returns whether `attr` is a struct-level helper consumed by `#[event]`
/// (a log-severity or metric-kind attribute) that must be stripped from the
/// re-emitted struct.
fn is_event_helper_attr(attr: &Attribute) -> bool {
    attr.path()
        .get_ident()
        .is_some_and(|ident| SeverityKind::from_ident(ident).is_some() || InstrumentKindValue::from_ident(ident).is_some())
}

/// Returns whether `attr` is a field-level helper (`#[dimension]`, `#[unredacted]`,
/// `#[data_class]`, `#[if_none]`) that must be stripped from the re-emitted struct.
fn is_field_helper_attr(attr: &Attribute) -> bool {
    ["dimension", "unredacted", "data_class", "if_none"]
        .iter()
        .any(|name| attr.path().is_ident(name))
}

/// Removes the `observed` helper attributes from the struct and its fields so the
/// re-emitted definition compiles without the (now consumed) attributes.
fn strip_helper_attrs(mut item: ItemStruct) -> ItemStruct {
    item.attrs.retain(|attr| !is_event_helper_attr(attr));
    let field_attrs = match &mut item.fields {
        Fields::Named(fields) => Some(fields.named.iter_mut()),
        Fields::Unnamed(fields) => Some(fields.unnamed.iter_mut()),
        Fields::Unit => None,
    };
    if let Some(fields) = field_attrs {
        for field in fields {
            field.attrs.retain(|attr| !is_field_helper_attr(attr));
        }
    }
    item
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
    // Using `split_once` (rather than index arithmetic) keeps the scan free of
    // off-by-one offsets.
    let mut rest = message;
    while let Some((_, after_open)) = rest.split_once('{') {
        // Escaped `{{` — skip the second brace and keep scanning.
        if let Some(stripped) = after_open.strip_prefix('{') {
            rest = stripped;
            continue;
        }
        // An unterminated `{` (no closing brace) is ignored.
        let Some((placeholder, remainder)) = after_open.split_once('}') else {
            break;
        };
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
        rest = remainder;
    }

    Ok(())
}

/// Builds the `Option<LogDescription>` expression for the event's log signal
/// (or `None` when the event declares no severity attribute).
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

/// Whether this field is routed to at least one signal.
///
/// [`generate_field_visit`] emits no code for a field that reaches neither, and
/// [`event_predicates`] adds no redaction bounds for one. Both call this so the
/// `where` clause always covers exactly the fields the body visits - when the
/// two drifted apart, the impl demanded bounds it never used and generic events
/// stopped compiling.
fn field_reaches_signal(field: &FieldDef, has_log: bool) -> bool {
    let routed_to_log = has_log && field.log_key().is_some();
    let routed_to_metric = field.metric_value.is_some() || field.metric_dimension.is_dimension();
    routed_to_log || routed_to_metric
}

/// Collects the `where` predicates the generated `Event` impl relies on.
///
/// `Event: Send + Sync` needs every generic field type to be `Send + Sync` -
/// the auto traits then carry `Self` - and each getter needs the bounds of its
/// own redaction path. Concrete field types are skipped; the compiler resolves
/// those without help.
///
/// The redaction bounds are added only for fields that reach a signal, since a
/// field routed nowhere generates no code to bound - which a metric-only event
/// does to every one of its non-dimension fields.
fn event_predicates(def: &EventDef) -> Vec<syn::WherePredicate> {
    let params = crate::field_attrs::type_param_idents(&def.generics);
    let has_log = def.log.is_some();
    let mut predicates = Vec::new();
    for field in &def.fields {
        if field_reaches_signal(field, has_log) {
            predicates.extend(crate::field_attrs::field_predicates(&field.ty, &field.redaction, &params));
        }
        let ty = &field.ty;
        if crate::field_attrs::mentions_any_type_param(ty, &params) {
            predicates.push(syn::parse_quote!(#ty: ::core::marker::Send + ::core::marker::Sync));
        }
    }
    predicates
}

fn generate_event_impl(def: &EventDef) -> TokenStream {
    let struct_ident = &def.ident;

    // Event identity comes from the required `#[event("...")]` attribute.
    let event_name = &def.event_name;

    // Add 'static bound to type parameters so TypeId::of works.
    let mut generics_with_static = def.generics.clone();
    for param in &mut generics_with_static.params {
        if let syn::GenericParam::Type(tp) = param {
            tp.bounds.push(syn::TypeParamBound::Lifetime(syn::Lifetime::new(
                "'static",
                proc_macro2::Span::call_site(),
            )));
        }
    }

    // Spell out the obligations the generated body relies on, so a generic event
    // reports them against the caller's type instead of failing inside the
    // expansion.
    crate::field_attrs::extend_where_clause(&mut generics_with_static, event_predicates(def));

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
    // Unraw-ed so a field written as `r#type` is exported under the domain key
    // `type`; `field_ident` keeps the raw form because it addresses the field.
    let default_key = field.ident.unraw().to_string();

    // Log routing
    let log_key = if has_log { field.log_key() } else { None };
    let log_entry = if let Some(key) = &log_key {
        quote! { ::core::option::Option::Some(::observed::metadata::LogFieldEntry::new(#key)) }
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
            ::core::option::Option::Some(::observed::metadata::MetricFieldEntry::instrument(
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
        quote! { ::core::option::Option::Some(::observed::metadata::MetricFieldEntry::dimension(#key)) }
    } else {
        quote! { ::core::option::Option::None }
    };

    // Skip emitting a visit if the field is not routed to any signal. The
    // `where` clause is built from the same predicate, so both must agree.
    if !field_reaches_signal(field, has_log) {
        return quote! {};
    }

    let field_desc = quote! {
        const FIELD_DESC: ::observed::metadata::FieldDescriptor =
            ::observed::metadata::FieldDescriptor::new(#default_key, #log_entry, #metric_entry);
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
    let value = value_expr(&field.redaction, &owned, &by_ref, &quote! { engine }, is_borrowed_str(&field.ty));
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

/// Builds the `::observed::Value::…` expression for a field value.
///
/// All generated paths are fully qualified: the consumer's `#[data_class(...)]`
/// expression is interpolated into this output, so importing `Value` here would
/// shadow a consumer type of the same name and break otherwise valid input.
///
/// `owned` are tokens evaluating to an owned `T` (for the `Into<Value>` path);
/// `by_ref` are tokens evaluating to `&T` (for both redaction paths); `engine`
/// names the redaction engine bound in the enclosing closure. The same helper
/// drives the non-optional path and both arms of an `Option<T>` field, so the
/// three redaction variants are defined once.
fn value_expr(
    redaction: &FieldRedaction,
    owned: &TokenStream,
    by_ref: &TokenStream,
    engine: &TokenStream,
    borrowed_str: bool,
) -> TokenStream {
    match redaction {
        // A borrowed `&str` cannot be stored by reference, so the macro spells
        // out the copy that `Value` has no `From<&str>` impl to hide.
        FieldRedaction::Unredacted if borrowed_str => {
            quote! { ::observed::Value::from(::std::sync::Arc::<str>::from(#by_ref)) }
        }
        FieldRedaction::Unredacted => quote! { ::observed::Value::from(#owned) },
        // `Sensitive<T>` is `RedactedDisplay` whenever `T: Display`, and a
        // reference to a `Display` type is itself `Display`, so the classified
        // value can be borrowed rather than cloned. `from_redacted` only
        // borrows the temporary `Sensitive`, so nothing needs to own the value.
        FieldRedaction::DataClass(expr) => quote! {
            ::observed::Value::from_redacted(
                &::observed::__private::Sensitive::new(#by_ref, #expr),
                #engine)
        },
        FieldRedaction::Default => quote! {
            ::observed::Value::from_redacted(#by_ref, #engine)
        },
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
    // For `Value::from_redacted` we need `&T`: when the inner type is already a reference
    // (`__val: &&T`) we deref once; otherwise `__val: &T` is used directly. The
    // owned form clones an owned inner, or copies the reference for a reference inner.
    let (val_owned, val_ref) = if inner_is_ref {
        (quote! { *__val }, quote! { *__val })
    } else {
        (quote! { __val.clone() }, quote! { __val })
    };
    let some_value = value_expr(&field.redaction, &val_owned, &val_ref, &engine, is_borrowed_str(inner_ty));

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
                        visitor(&FIELD_DESC, &|_| ::observed::Value::from(#placeholder))?;
                    }
                }
            }
        },
    }
}

// miri fails to use insta snapshots: `insta::_macro_support::get_cargo_workspace` leads to
#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(all(test, not(miri)))]
mod tests {
    use super::*;

    /// Expands a new-syntax event struct (including its `#[event(...)]` attribute)
    /// into just the generated `Event` impl, mirroring what the attribute macro
    /// emits alongside the re-emitted struct.
    fn run(input: &str) -> Result<TokenStream> {
        let item: TokenStream = input.parse().expect("failed to tokenize input");
        let mut item_struct: ItemStruct = syn::parse2(item)?;
        let mut event_args: Option<EventArgs> = None;
        let mut kept = Vec::with_capacity(item_struct.attrs.len());
        for attr in item_struct.attrs.drain(..) {
            if attr.path().is_ident("event") {
                event_args = Some(attr.parse_args::<EventArgs>()?);
            } else {
                kept.push(attr);
            }
        }
        item_struct.attrs = kept;
        let args = event_args.ok_or_else(|| Error::new_spanned(&item_struct.ident, "missing `#[event(...)]` attribute"))?;
        generate_event(
            &item_struct.ident,
            &item_struct.generics,
            &item_struct.attrs,
            &item_struct.fields,
            &args,
        )
    }

    fn parse_and_generate(input: &str) -> String {
        let tokens = run(input).expect("failed to generate");
        let file = syn::parse2(tokens).expect("failed to parse generated code");
        prettyplease::unparse(&file)
    }

    fn parse_and_expect_error(input: &str) -> String {
        run(input).expect_err("expected generation to fail").to_string()
    }

    #[test]
    fn test_basic_event() {
        let output = parse_and_generate(
            r#"
            #[event("http.request")]
            #[info]
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
            #[event("request.failed")]
            #[warning("Request failed")]
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
            #[event("my.event")]
            #[info]
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
            #[event("outgoing_request")]
            #[info("Outgoing request")]
            #[histogram(duration, name = "request_duration", unit = "ms")]
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
            #[event("http.outgoing_request")]
            #[error("Outgoing HTTP request")]
            #[counter(name = "http.request.count")]
            #[histogram(duration, name = "request_duration")]
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
            #[event("debug.diagnostics", disabled)]
            #[debug("Internal diagnostics")]
            #[gauge(queue_depth_metric, name = "debug.queue_depth")]
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
            #[event("user.login")]
            #[info]
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
            #[event("bytes.received")]
            #[counter(bytes, name = "bytes.received.total", unit = "By")]
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
            #[event("queue.delta")]
            #[updown_counter(delta, name = "queue.size.delta")]
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
            #[event("http.request.count")]
            #[counter]
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
            #[event("system.memory")]
            #[gauge(bytes, name = "system.memory.usage")]
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
            #[event("bad")]
            #[info]
            enum BadEvent { A, B }
            "#,
        );
        assert!(err.contains("struct"), "{err}");
    }

    #[test]
    fn test_no_signal() {
        let output = parse_and_generate(
            r#"
            #[event("no.signal")]
            struct NoSignal { x: String }
            "#,
        );
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_error_data_class_and_unredacted() {
        let err = parse_and_expect_error(
            r#"
            #[event("bad")]
            #[info]
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
            #[event("borrowed.event")]
            #[info]
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
            #[event("generic.event")]
            #[info]
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
            #[event("bounded.event")]
            #[info]
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
            #[event("bad")]
            #[info]
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
            #[event("bad")]
            #[info]
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
        // Without an `#[event(...)]` attribute there is no event to generate.
        let err = parse_and_expect_error(
            r"
            #[info]
            struct MissingEventName {
                #[unredacted]
                x: i64,
            }
            ",
        );
        assert!(err.contains("event"), "{err}");
    }

    #[test]
    fn test_log_name_override() {
        let output = parse_and_generate(
            r#"
            #[event("http.request")]
            #[info(name = "http.request.log")]
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
            #[event("bad")]
            #[info]
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
            #[event("bad")]
            #[info]
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
            #[event("bad")]
            #[info]
            struct BadEvent {
                #[unredacted(foo)]
                x: String,
            }
            "#,
        );
        assert!(err.contains("does not accept arguments"), "{err}");
    }

    /// `#[unredacted]` is a marker, so a name-value form must be rejected
    /// rather than silently selecting the privacy bypass.
    #[test]
    fn test_error_unredacted_with_value() {
        for payload in ["false", "\"no\"", "0"] {
            let err = parse_and_expect_error(&format!(
                r#"
                #[event("bad")]
                #[info]
                struct BadEvent {{
                    #[unredacted = {payload}]
                    x: String,
                }}
                "#,
            ));
            assert!(err.contains("does not accept arguments"), "{err}");
        }
    }

    #[test]
    fn test_error_log_unknown_option_rejected() {
        // A log-severity attribute only accepts a message and `name = "..."`.
        let err = parse_and_expect_error(
            r#"
            #[event("bad")]
            #[info(target = "svc")]
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
            #[event("bad")]
            #[info("Hello {missing}")]
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
            #[event("bad")]
            #[info("Value: {my_field}")]
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
            #[event("good")]
            #[info("Value: {custom_name}")]
            struct GoodEvent {
                #[dimension(log = "custom_name")]
                #[unredacted]
                my_field: i64,
            }
            "#,
        );
    }

    #[test]
    fn bare_severity_generates_without_body() {
        // A severity attribute with no arguments (`#[info]`) opts into logging
        // with no message body.
        let _output = parse_and_generate(
            r#"
            #[event("e")]
            #[info]
            struct E {
                #[unredacted]
                x: i64,
            }
            "#,
        );
    }

    #[test]
    fn metric_error_names_the_instrument_kind() {
        // A fieldless non-counter metric is rejected, and the message must
        // spell out the offending kind via `InstrumentKindValue::attr_name`.
        let err = parse_and_expect_error(
            r#"
            #[event("e")]
            #[gauge]
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
            #[event("e")]
            #[info("Value: {nonexistent}")]
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
            #[event("bad")]
            #[info("Tag: {tag}")]
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
            #[event("workload.disabled")]
            #[info]
            struct NoV2Workloads;
            "#,
        );
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_event_with_reference_to_redactable_type() {
        let output = parse_and_generate(
            r#"
            #[event("borrowed.classified")]
            #[info]
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
            #[event("paren.ref")]
            #[info]
            struct ParenRef<'a> {
                name: (&'a PiiString),
            }
            "#,
        );
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_option_field_filled_when_none() {
        // By default a `None` `Option<T>` is filled with the `"n/a"` placeholder.
        let output = parse_and_generate(
            r#"
            #[event("http.request")]
            #[info]
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
            #[event("http.request")]
            #[info]
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
            #[event("http.request")]
            #[info]
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
            #[event("http.request.count")]
            #[counter]
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
            #[event("bad")]
            #[info]
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
            #[event("bad")]
            #[counter(count)]
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
            #[event("bad")]
            #[updown_counter(delta)]
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
            #[event("bad")]
            #[counter(count)]
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
            #[event("bad")]
            #[histogram(nope)]
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
            #[event("bad")]
            #[gauge(name = "x")]
            struct BadEvent {
                #[unredacted]
                value: f64,
            }
            "#,
        );
        assert!(err.contains("requires a field"), "{err}");
    }

    #[test]
    fn test_error_updowncounter_requires_field() {
        let err = parse_and_expect_error(
            r#"
            #[event("bad")]
            #[updown_counter(name = "x")]
            struct BadEvent {
                #[unredacted]
                value: i64,
            }
            "#,
        );
        assert!(err.contains("requires a field"), "{err}");
    }

    #[test]
    fn test_error_instrument_on_field() {
        let err = parse_and_expect_error(
            r#"
            #[event("bad")]
            struct BadEvent {
                #[counter(x)]
                #[unredacted]
                x: u64,
            }
            "#,
        );
        assert!(err.contains("struct-level attribute"), "{err}");
    }

    #[test]
    fn test_error_field_both_metric_and_dimension() {
        let err = parse_and_expect_error(
            r#"
            #[event("bad")]
            #[histogram(duration)]
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
            #[event("bad")]
            #[histogram(duration)]
            #[gauge(duration)]
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
            #[event("bad")]
            #[counter]
            #[counter(name = "other")]
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
            #[event("http.request.count")]
            #[info]
            #[counter]
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
            #[event("http.request")]
            #[info]
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
            #[event("http.request.count")]
            #[info]
            #[counter]
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
            #[event("http.request.count")]
            #[info]
            #[counter]
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
            #[event("http.request.count")]
            #[info]
            #[counter]
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
            #[event("http.request.count")]
            #[info]
            #[counter]
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
            #[event("http.request.count")]
            #[info]
            #[counter]
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
            #[event("http.request")]
            #[info]
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
            #[event("outgoing_request")]
            #[info]
            #[histogram(duration)]
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
            #[event("bad")]
            #[info]
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
            #[event("bad")]
            #[info]
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
            #[event("bad")]
            #[info]
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

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(all(test, not(miri)))]
mod coverage_tests {
    use super::*;

    /// Expands `input` (a new-syntax event struct including its `#[event(...)]`
    /// attribute) into just the generated impl, surfacing any error.
    #[cfg_attr(test, mutants::skip)] // test-only helper: mutating it can't change observable behavior
    fn expand(input: &str) -> Result<TokenStream> {
        let item: TokenStream = input.parse().expect("failed to tokenize input");
        let mut item_struct: ItemStruct = syn::parse2(item)?;
        let mut event_args: Option<EventArgs> = None;
        let mut kept = Vec::with_capacity(item_struct.attrs.len());
        for attr in item_struct.attrs.drain(..) {
            if attr.path().is_ident("event") {
                event_args = Some(attr.parse_args::<EventArgs>()?);
            } else {
                kept.push(attr);
            }
        }
        item_struct.attrs = kept;
        let args = event_args.ok_or_else(|| Error::new_spanned(&item_struct.ident, "missing `#[event(...)]` attribute"))?;
        generate_event(
            &item_struct.ident,
            &item_struct.generics,
            &item_struct.attrs,
            &item_struct.fields,
            &args,
        )
    }

    /// Parses `input` and asserts generation fails, returning the error text.
    #[cfg_attr(test, mutants::skip)] // test-only helper: mutating it can't change observable behavior
    fn expect_err(input: &str) -> String {
        expand(input).expect_err("expected generation to fail").to_string()
    }

    /// Parses `input` and asserts generation succeeds.
    #[cfg_attr(test, mutants::skip)] // test-only helper: mutating it can't change observable behavior
    fn expect_ok(input: &str) {
        let tokens = expand(input).expect("expected generation to succeed");
        syn::parse2::<syn::File>(tokens).expect("generated code should parse");
    }

    #[test]
    fn tuple_struct_is_rejected() {
        let _ = expect_err(r#"#[event("e")] struct E(i64);"#);
    }

    #[test]
    fn union_is_rejected() {
        let _ = expect_err(r#"#[event("e")] union E { a: i64 }"#);
    }

    #[test]
    fn duplicate_log_attribute_is_rejected() {
        let _ = expect_err(r#"#[event("e")] #[info] #[warning] struct E { #[unredacted] v: i64 }"#);
    }

    #[test]
    fn message_escaped_brace_and_valid_placeholder() {
        // Escaped `{{` is skipped while a valid `{name}` placeholder resolves.
        expect_ok(r#"#[event("e")] #[info("a {{ b {name}")] struct E { #[unredacted] name: i64 }"#);
    }

    #[test]
    fn const_generic_event_generates() {
        expect_ok(r#"#[event("e")] #[info] struct E<const N: usize> { #[unredacted] v: i64 }"#);
    }

    #[test]
    fn dimension_name_value_is_rejected() {
        let _ = expect_err(r#"#[event("e")] #[info] struct E { #[dimension = 1] #[unredacted] v: i64 }"#);
    }

    #[test]
    fn if_none_without_argument_is_rejected() {
        let _ = expect_err(r#"#[event("e")] #[info] struct E { #[if_none] #[unredacted] v: Option<i64> }"#);
    }

    #[test]
    fn if_none_bad_keyword_is_rejected() {
        let _ = expect_err(r#"#[event("e")] #[info] struct E { #[if_none(bogus)] #[unredacted] v: Option<i64> }"#);
    }

    #[test]
    fn duplicate_if_none_is_rejected() {
        let _ = expect_err(r#"#[event("e")] #[info] struct E { #[if_none(drop)] #[if_none("x")] #[unredacted] v: Option<i64> }"#);
    }

    #[test]
    fn data_class_after_unredacted_is_rejected() {
        let _ = expect_err(r#"#[event("e")] #[info] struct E { #[unredacted] #[data_class(Foo::Bar)] v: i64 }"#);
    }

    #[test]
    fn option_field_with_reference_inner_generates() {
        // A `Option<&T>` field drives the `inner_is_ref` branch of the option
        // visit codegen.
        expect_ok(r#"#[event("e")] #[info] struct E { #[unredacted] v: Option<&'static str> }"#);
    }

    #[test]
    fn message_with_unterminated_brace_is_ignored() {
        // A `{` with no matching `}` is skipped rather than treated as a placeholder.
        expect_ok(r#"#[event("e")] #[info("x {y")] struct E { #[unredacted] y: i64 }"#);
    }

    #[test]
    fn classified_metric_value_field_is_rejected() {
        // A classified value is rendered through the redaction engine as a
        // string, which carries no measurement -- so it must be a compile-time
        // error rather than an instrument that silently records nothing.
        let msg = expect_err(r#"#[event("e")] #[info] #[counter(n)] struct E { #[data_class(DC)] n: u64 }"#);
        assert!(msg.contains("#[unredacted]"), "diagnostic should name the fix, got: {msg}");

        // The default (no redaction attribute at all) takes the same path.
        let msg = expect_err(r#"#[event("e")] #[info] #[counter(n)] struct E { n: u64 }"#);
        assert!(msg.contains("#[unredacted]"), "diagnostic should name the fix, got: {msg}");
    }

    #[test]
    fn non_numeric_metric_value_field_is_rejected() {
        // `gauge`/`histogram` place no signedness constraint, but they still
        // need a number: a non-numeric value produces no measurement at all.
        for kind in ["gauge", "histogram"] {
            let msg = expect_err(&format!(
                r#"#[event("e")] #[info] #[{kind}(n)] struct E {{ #[unredacted] n: PublicString }}"#
            ));
            assert!(msg.contains("numeric"), "diagnostic should say numeric, got: {msg}");
        }
    }

    #[test]
    fn metric_value_field_accepts_supported_widths() {
        // `u64` in particular: it is the natural type for a byte or request
        // counter, and `Value` carries it exactly.
        for ty in ["u8", "u16", "u32", "u64", "usize"] {
            expect_ok(&format!(
                r#"#[event("e")] #[info] #[counter(n)] struct E {{ #[unredacted] n: {ty} }}"#
            ));
        }
        for ty in ["i8", "i16", "i32", "i64", "isize"] {
            expect_ok(&format!(
                r#"#[event("e")] #[info] #[updown_counter(n)] struct E {{ #[unredacted] n: {ty} }}"#
            ));
        }
        // Gauge and histogram are signedness- and width-agnostic, floats included.
        for ty in ["u64", "i64", "f32", "f64"] {
            expect_ok(&format!(
                r#"#[event("e")] #[info] #[gauge(n)] struct E {{ #[unredacted] n: {ty} }}"#
            ));
            expect_ok(&format!(
                r#"#[event("e")] #[info] #[histogram(n)] struct E {{ #[unredacted] n: {ty} }}"#
            ));
        }
    }

    #[test]
    fn metric_value_field_rejects_128_bit_widths() {
        // No telemetry backend represents these, so `Value` has no conversion.
        // The diagnostic is specific rather than the generic "not numeric" one.
        let msg = expect_err(r#"#[event("e")] #[info] #[counter(n)] struct E { #[unredacted] n: u128 }"#);
        assert!(msg.contains("128-bit"), "diagnostic should call out the width, got: {msg}");
        let msg = expect_err(r#"#[event("e")] #[info] #[updown_counter(n)] struct E { #[unredacted] n: i128 }"#);
        assert!(msg.contains("128-bit"), "diagnostic should call out the width, got: {msg}");
    }

    #[test]
    fn counter_still_requires_unsigned_and_updown_counter_signed() {
        let msg = expect_err(r#"#[event("e")] #[info] #[counter(n)] struct E { #[unredacted] n: i64 }"#);
        assert!(msg.contains("unsigned"), "got: {msg}");
        let msg = expect_err(r#"#[event("e")] #[info] #[updown_counter(n)] struct E { #[unredacted] n: u64 }"#);
        assert!(msg.contains("signed"), "got: {msg}");
        // A float is not an integer, so neither counter accepts one.
        let msg = expect_err(r#"#[event("e")] #[info] #[counter(n)] struct E { #[unredacted] n: f64 }"#);
        assert!(msg.contains("unsigned"), "got: {msg}");
    }

    #[test]
    fn numeric_kind_of_non_path_type_is_none() {
        let reference: syn::Type = syn::parse_str("&u64").expect("parse type");
        assert!(numeric_kind(&reference).is_none());
    }

    #[test]
    fn numeric_kind_does_not_see_through_option() {
        // `Option<u64>` must not satisfy an instrument's value-type requirement --
        // an optional field has no measurement to record when it is `None`.
        let optional: syn::Type = syn::parse_str("Option<u64>").expect("parse type");
        assert!(numeric_kind(&optional).is_none());
    }

    #[test]
    fn numeric_kind_rejects_128_bit_widths() {
        // `Value` has no conversion for these, so they are not "numeric" for the
        // purposes of an instrument; `is_128_bit_int` exists to give them a
        // dedicated diagnostic instead of the generic one.
        for spelling in ["u128", "i128"] {
            let ty: syn::Type = syn::parse_str(spelling).expect("parse type");
            assert!(numeric_kind(&ty).is_none(), "{spelling} must not be a supported numeric");
            assert!(is_128_bit_int(&ty), "{spelling} must be recognized for its own diagnostic");
        }
    }

    #[test]
    fn is_128_bit_int_is_false_for_non_path_and_supported_types() {
        let reference: syn::Type = syn::parse_str("&u128").expect("parse type");
        assert!(!is_128_bit_int(&reference));
        let supported: syn::Type = syn::parse_str("u64").expect("parse type");
        assert!(!is_128_bit_int(&supported));
    }

    #[test]
    fn numeric_kind_classifies_floats() {
        for spelling in ["f32", "f64"] {
            let ty: syn::Type = syn::parse_str(spelling).expect("parse type");
            assert!(matches!(numeric_kind(&ty), Some(NumericKind::Float)));
        }
    }

    #[test]
    fn optional_metric_value_field_is_rejected() {
        // An instrument records a measurement on every emission, so its value field
        // cannot be `Option<T>`: `#[if_none(...)]` would fill `None` with a placeholder
        // string, which is not a valid measurement.
        for attr in ["counter(v)", "updown_counter(v)", "gauge(v)", "histogram(v)"] {
            let err = expect_err(&format!(r#"#[event("e")] #[{attr}] struct E {{ #[unredacted] v: Option<u64> }}"#));
            assert!(
                err.contains("a metric value cannot be optional"),
                "unexpected error for `{attr}`: {err}"
            );
        }
    }

    #[test]
    fn optional_metric_dimension_field_is_still_allowed() {
        // Only the metric *value* is constrained; an optional dimension is fine
        // because a placeholder is a meaningful attribute value.
        expect_ok(r#"#[event("e")] #[info] struct E { #[dimension(metric)] #[unredacted] v: Option<u64> }"#);
    }

    #[test]
    fn strip_type_wrappers_unwraps_paren_and_group() {
        // `Paren` comes from source; `Group` is synthesized (it only appears from
        // macro-expanded token streams, never hand-written source).
        let paren: syn::Type = syn::parse_str("(u64)").expect("parse type");
        assert!(matches!(strip_type_wrappers(&paren), syn::Type::Path(_)));

        let inner: syn::Type = syn::parse_str("u64").expect("parse type");
        let grouped = syn::Type::Group(syn::TypeGroup {
            attrs: Vec::new(),
            group_token: syn::token::Group::default(),
            elem: Box::new(inner),
        });
        assert!(matches!(strip_type_wrappers(&grouped), syn::Type::Path(_)));
    }

    #[test]
    fn event_args_requires_a_string_name() {
        let err = syn::parse_str::<EventArgs>("123")
            .err()
            .expect("a non-string event name must be rejected");
        assert!(err.to_string().contains("requires a string event name"), "unexpected error: {err}");
    }

    #[test]
    fn event_args_accepts_trailing_comma() {
        let args = syn::parse_str::<EventArgs>(r#""e","#).expect("a trailing comma is allowed");
        assert_eq!(args.name, "e");
        assert!(!args.disabled);
    }

    #[test]
    fn event_args_rejects_unknown_flag() {
        let err = syn::parse_str::<EventArgs>(r#""e", bogus"#)
            .err()
            .expect("an unknown flag must be rejected");
        assert!(
            err.to_string().contains("unknown `#[event(...)]` flag `bogus`"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn log_message_followed_by_named_option_generates() {
        // The comma between the positional message and `name = "..."` drives the
        // separator branch of the log attribute body parser.
        expect_ok(r#"#[event("e")] #[info("m", name = "log.name")] struct E { #[unredacted] v: i64 }"#);
    }

    #[test]
    fn duplicate_log_name_option_is_rejected() {
        let err = expect_err(r#"#[event("e")] #[info("m", name = "a", name = "b")] struct E { #[unredacted] v: i64 }"#);
        assert!(err.contains("duplicate `name` setting"), "unexpected error: {err}");
    }

    #[test]
    fn unknown_metric_option_is_rejected() {
        let err = expect_err(r#"#[event("e")] #[counter(bogus = "x")] struct E { #[unredacted] v: i64 }"#);
        assert!(err.contains("unknown metric option `bogus`"), "unexpected error: {err}");
    }

    #[test]
    fn non_ident_struct_attribute_is_ignored() {
        // A multi-segment attribute path has no single ident, so it can be neither
        // a log-severity nor a metric-kind helper and is skipped.
        expect_ok(r#"#[event("e")] #[info] #[some::other] struct E { #[unredacted] v: i64 }"#);
    }

    #[test]
    fn log_attribute_written_as_name_value_is_rejected() {
        let err = expect_err(r#"#[event("e")] #[info = "m"] struct E { #[unredacted] v: i64 }"#);
        assert!(err.contains("not `= value`"), "unexpected error: {err}");
    }

    #[test]
    fn metric_attribute_written_as_name_value_is_rejected() {
        let err = expect_err(r#"#[event("e")] #[counter = "c"] struct E { #[unredacted] v: i64 }"#);
        assert!(err.contains("not `= value`"), "unexpected error: {err}");
    }

    #[test]
    fn event_attr_reemits_struct_without_helper_attributes() {
        // The attribute entry point re-emits the struct alongside the generated
        // impl, with every consumed helper attribute stripped and every
        // unrelated attribute left untouched.
        let attr: TokenStream = r#""http.request""#.parse().expect("failed to tokenize attribute");
        let item: TokenStream = r#"#[derive(Debug)] #[info("hi")] struct E { #[allow(dead_code)] #[unredacted] v: i64 }"#
            .parse()
            .expect("failed to tokenize item");

        let expanded = event_attr(attr, item).expect("attribute expansion should succeed");

        let file: syn::File = syn::parse2(expanded).expect("generated code should parse");
        let syn::Item::Struct(reemitted) = &file.items[0] else {
            panic!("the first generated item should be the re-emitted struct");
        };
        let struct_attrs: Vec<_> = reemitted
            .attrs
            .iter()
            .map(|a| a.path().get_ident().map(ToString::to_string))
            .collect();
        assert_eq!(
            struct_attrs,
            vec![Some("derive".to_owned())],
            "`#[info]` should be stripped and `#[derive]` preserved"
        );

        let field_attrs: Vec<_> = reemitted
            .fields
            .iter()
            .flat_map(|f| &f.attrs)
            .map(|a| a.path().get_ident().map(ToString::to_string))
            .collect();
        assert_eq!(
            field_attrs,
            vec![Some("allow".to_owned())],
            "`#[unredacted]` should be stripped and `#[allow]` preserved"
        );
    }

    #[test]
    fn event_attr_propagates_codegen_errors() {
        // The entry point must surface codegen failures rather than re-emitting
        // a struct with no `Event` impl.
        let attr: TokenStream = r#""e""#.parse().expect("failed to tokenize attribute");
        let item: TokenStream = "struct E(i64);".parse().expect("failed to tokenize item");

        let err = event_attr(attr, item).expect_err("a tuple struct must be rejected");

        assert!(err.to_string().contains("named fields"), "unexpected error: {err}");
    }

    #[test]
    fn strip_helper_attrs_clears_tuple_struct_field_attributes() {
        // Tuple structs are rejected by codegen, but the re-emit helper stays
        // total over `Fields` so it never silently leaves helpers behind.
        let item: ItemStruct = syn::parse_str(r"#[info] struct E(#[unredacted] i64);").expect("failed to parse tuple struct");

        let stripped = strip_helper_attrs(item);

        assert!(stripped.attrs.is_empty());
        assert!(stripped.fields.iter().all(|field| field.attrs.is_empty()));
    }

    #[test]
    fn every_severity_attribute_maps_to_its_own_variant() {
        // A dropped arm would leave the attribute unrecognized, silently
        // demoting the event to "no log signal" instead of failing to build.
        for (attribute, expected) in [
            ("trace", "Trace"),
            ("debug", "Debug"),
            ("info", "Info"),
            ("warning", "Warn"),
            ("error", "Error"),
            ("fatal", "Fatal"),
        ] {
            let parsed = SeverityKind::from_ident(&format_ident!("{attribute}")).expect("known severity attribute");
            assert_eq!(
                parsed.to_token_stream().to_string(),
                expected,
                "`#[{attribute}]` mapped to the wrong severity"
            );
        }

        assert!(SeverityKind::from_ident(&format_ident!("bogus")).is_none());
    }
}
