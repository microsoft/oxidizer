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
#[derive(Debug)]
pub(crate) struct EventArgs {
    pub name: String,
    pub disabled: bool,
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

#[derive(Clone, Copy, Debug)]
pub(crate) enum SeverityKind {
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
    #[must_use]
    pub(crate) fn from_ident(ident: &Ident) -> Option<Self> {
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
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum NumericKind {
    /// A signed integer: `i8`, `i16`, `i32`, `i64`, `isize`.
    SignedInt,
    /// An unsigned integer: `u8`, `u16`, `u32`, `u64`, `usize`.
    UnsignedInt,
    /// A float: `f32`, `f64`.
    Float,
}

/// Classifies a primitive numeric type, matched syntactically on the last path
/// segment (so `u64` and `std::primitive::u64` are recognized, but a type
/// aliased to an integer is not).
///
/// Returns `None` for non-numeric types,
/// unrecognized paths, and the unsupported 128-bit widths. Group and
/// parenthesis wrappers are transparent; `Option<T>` deliberately is **not**, so
/// an optional field can never satisfy an instrument's value-type requirement.
#[must_use]
pub(crate) fn numeric_kind(ty: &syn::Type) -> Option<NumericKind> {
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
#[must_use]
pub(crate) fn is_128_bit_int(ty: &syn::Type) -> bool {
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
#[must_use]
pub(crate) fn strip_type_wrappers(ty: &syn::Type) -> &syn::Type {
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
pub(crate) fn event_attr(attr: TokenStream, item: TokenStream, runtime: &TokenStream) -> Result<TokenStream> {
    let args: EventArgs = syn::parse2(attr)?;
    let item_struct: ItemStruct = syn::parse2(item)?;
    let impl_tokens = generate_event(
        &item_struct.ident,
        &item_struct.generics,
        &item_struct.attrs,
        &item_struct.fields,
        &args,
        runtime,
    )?;
    let cleaned = strip_helper_attrs(item_struct);
    Ok(quote! {
        #cleaned
        #impl_tokens
    })
}

/// Generates just the `Event` trait impl (without re-emitting the struct).
pub(crate) fn generate_event(
    ident: &Ident,
    generics: &Generics,
    attrs: &[Attribute],
    fields: &Fields,
    args: &EventArgs,
    runtime: &TokenStream,
) -> Result<TokenStream> {
    let def = parse_event_def(ident, generics, attrs, fields, args)?;
    validate_message_placeholders(&def)?;
    Ok(generate_event_impl(&def, runtime))
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
#[must_use]
pub(crate) fn strip_helper_attrs(mut item: ItemStruct) -> ItemStruct {
    item.attrs.retain(|attr| !is_event_helper_attr(attr));
    // `parse_event_def` rejects a tuple struct, and `event_attr` propagates that before reaching
    // here, so named fields (or none at all) are the only shapes that arrive.
    if let Fields::Named(fields) = &mut item.fields {
        for field in &mut fields.named {
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
fn log_description_expr(def: &EventDef, runtime: &TokenStream) -> TokenStream {
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
            #runtime::metadata::LogDescription::new(
                #log_name,
                #runtime::Severity::#severity,
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
fn event_predicates(def: &EventDef, runtime: &TokenStream) -> Vec<syn::WherePredicate> {
    let params = crate::field_attrs::type_param_idents(&def.generics);
    let has_log = def.log.is_some();
    let mut predicates = Vec::new();
    for field in &def.fields {
        if field_reaches_signal(field, has_log) {
            predicates.extend(crate::field_attrs::field_predicates(&field.ty, &field.redaction, &params, runtime));
        }
        let ty = &field.ty;
        if crate::field_attrs::mentions_any_type_param(ty, &params) {
            predicates.push(syn::parse_quote!(#ty: ::core::marker::Send + ::core::marker::Sync));
        }
    }
    predicates
}

fn generate_event_impl(def: &EventDef, runtime: &TokenStream) -> TokenStream {
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
    crate::field_attrs::extend_where_clause(&mut generics_with_static, event_predicates(def, runtime));

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

    let log_expr = log_description_expr(def, runtime);

    let metric_expr = if let Some(metric) = &def.metric {
        let instrument_name = metric.name.clone().unwrap_or_else(|| def.event_name.clone());
        let description = metric.description.as_deref().unwrap_or("");
        let unit = metric.unit.as_deref().unwrap_or("");
        // A fieldless event-level metric is always a counter (records `1`).
        quote! {
            ::core::option::Option::Some(
                #runtime::metadata::MetricDescription::new(
                    #instrument_name,
                    #runtime::metadata::InstrumentKind::Counter,
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

    let visit_fields_body = generate_visit_fields_body(&def.fields, has_log, runtime);

    quote! {
        const _: () = {
            impl #impl_generics #runtime::Event for #struct_ident #ty_generics #where_clause {
                const DESCRIPTION: #runtime::metadata::EventDescription =
                    #runtime::metadata::EventDescription::new(
                        #event_name,
                        #type_id_expr,
                        #log_expr,
                        #metric_expr,
                        #has_field_metrics,
                        #disabled,
                    );

                fn visit_fields(
                    &self,
                    visitor: &mut #runtime::processing::FieldVisitorFn<'_>,
                ) -> ::core::ops::ControlFlow<()> {
                    #visit_fields_body
                    ::core::ops::ControlFlow::Continue(())
                }
            }
        };
    }
}

fn generate_visit_fields_body(fields: &[FieldDef], has_log: bool, runtime: &TokenStream) -> TokenStream {
    let visits: Vec<TokenStream> = fields.iter().map(|f| generate_field_visit(f, has_log, runtime)).collect();
    quote! { #(#visits)* }
}

fn generate_field_visit(field: &FieldDef, has_log: bool, runtime: &TokenStream) -> TokenStream {
    let field_ident = &field.ident;
    // Unraw-ed so a field written as `r#type` is exported under the domain key
    // `type`; `field_ident` keeps the raw form because it addresses the field.
    let default_key = field.ident.unraw().to_string();

    // Log routing
    let log_key = if has_log { field.log_key() } else { None };
    let log_entry = if let Some(key) = &log_key {
        quote! { ::core::option::Option::Some(#runtime::metadata::LogFieldEntry::new(#key)) }
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
            ::core::option::Option::Some(#runtime::metadata::MetricFieldEntry::instrument(
                #default_key,
                #runtime::metadata::MetricDescription::new(
                    #name,
                    #runtime::metadata::InstrumentKind::#kind,
                    #description,
                    #unit,
                ),
            ))
        }
    } else if let Some(key) = field.metric_dimension.resolve_key(&default_key) {
        quote! { ::core::option::Option::Some(#runtime::metadata::MetricFieldEntry::dimension(#key)) }
    } else {
        quote! { ::core::option::Option::None }
    };

    // Skip emitting a visit if the field is not routed to any signal. The
    // `where` clause is built from the same predicate, so both must agree.
    if !field_reaches_signal(field, has_log) {
        return quote! {};
    }

    let field_desc = quote! {
        const FIELD_DESC: #runtime::metadata::FieldDescriptor =
            #runtime::metadata::FieldDescriptor::new(#default_key, #log_entry, #metric_entry);
    };

    // `Option<T>` fields dispatch on `#[if_none(...)]`: a `None` value is
    // either dropped or replaced with a placeholder string (default `"n/a"`).
    if let Some(inner_ty) = option_inner_type(&field.ty) {
        return generate_option_field_visit(field, inner_ty, &field_desc, runtime);
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
    let value = value_expr(
        &field.redaction,
        &owned,
        &by_ref,
        &quote! { engine },
        is_borrowed_str(&field.ty),
        runtime,
    );
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

/// Builds the `Value::…` expression for a field value, rooted at the resolved
/// `observed` runtime path.
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
    runtime: &TokenStream,
) -> TokenStream {
    match redaction {
        // A borrowed `&str` cannot be stored by reference, so the macro spells
        // out the copy that `Value` has no `From<&str>` impl to hide.
        FieldRedaction::Unredacted if borrowed_str => {
            quote! { #runtime::Value::from(::std::sync::Arc::<str>::from(#by_ref)) }
        }
        FieldRedaction::Unredacted => quote! { #runtime::Value::from(#owned) },
        // `Sensitive<T>` is `RedactedDisplay` whenever `T: Display`, and a
        // reference to a `Display` type is itself `Display`, so the classified
        // value can be borrowed rather than cloned. `from_redacted` only
        // borrows the temporary `Sensitive`, so nothing needs to own the value.
        FieldRedaction::DataClass(expr) => quote! {
            #runtime::Value::from_redacted(
                &#runtime::__private::Sensitive::new(#by_ref, #expr),
                #engine)
        },
        FieldRedaction::Default => quote! {
            #runtime::Value::from_redacted(#by_ref, #engine)
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
fn generate_option_field_visit(field: &FieldDef, inner_ty: &syn::Type, field_desc: &TokenStream, runtime: &TokenStream) -> TokenStream {
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
    let some_value = value_expr(&field.redaction, &val_owned, &val_ref, &engine, is_borrowed_str(inner_ty), runtime);

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
                        visitor(&FIELD_DESC, &|_| #runtime::Value::from(#placeholder))?;
                    }
                }
            }
        },
    }
}
