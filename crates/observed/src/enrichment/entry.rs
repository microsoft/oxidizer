// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Enrichment data model: [`EnrichmentEntry`] and redaction helpers.

use std::fmt;
use std::sync::Arc;

use crate::key::Key;
use crate::sink::SinkId;
use crate::value::Value;

/// Internal storage for enrichment values.
///
/// Two variants support different enrichment patterns:
/// - [`Primitive`](UnredactedValue::Primitive): a pre-converted [`Value`] that passes through
///   without redaction. Used for unclassified primitives (`i64`, `f64`, `bool`, etc.).
/// - [`Unredacted`](UnredactedValue::Unredacted): the original classified value stored as a trait
///   object, redacted through [`RedactedDisplay`](data_privacy::RedactedDisplay) at emission time.
///   Used by [`new()`](EnrichmentEntry::new), `#[derive(Enrichment)]`, and
///   `Sensitive<T>`.
#[derive(Clone)]
enum UnredactedValue {
    /// Primitive value that passes through without redaction.
    Primitive(Value),
    /// Classified value stored as a trait object, redacted at emission time
    /// via [`RedactedDisplay`](data_privacy::RedactedDisplay).
    Unredacted(Arc<dyn data_privacy::RedactedDisplay + Send + Sync>),
}

impl fmt::Debug for UnredactedValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Primitive(value) => f.debug_tuple("Primitive").field(value).finish(),
            Self::Unredacted(_) => f.write_str("Unredacted(<classified>)"),
        }
    }
}

/// A single enrichment entry: a key-value pair with data classification.
///
/// Every enrichment carries privacy metadata so that string values are redacted
/// through the [`data_privacy::RedactionEngine`] at emission time, matching the
/// privacy guarantees of event fields.
///
/// For classified newtypes (`#[classified(...)]`), use [`new()`](Self::new) -
/// no `Into<Value>` impl required. For dynamically classified values, use
/// `Sensitive<V>` with the tuple conversion. For primitives, use
/// [`unclassified()`](Self::unclassified).
#[derive(Debug, Clone)]
pub struct EnrichmentEntry {
    /// The enrichment key.
    key: Key,
    /// The enrichment value - either pre-resolved or deferred for emission-time redaction.
    stored: UnredactedValue,
    /// When `true`, this entry is excluded from log records.
    exclude_from_logs: bool,
    /// When `Some`, this entry is a metric dimension with the given key.
    /// Metric dimensions are opt-in via `#[dimension]`, mirroring events.
    metric_key: Option<Key>,
    /// If `Some`, this entry only applies to the specified sink (targeted enrichment).
    /// If `None`, this is a global enrichment visible to all emitters.
    target: Option<SinkId>,
}

impl EnrichmentEntry {
    /// Creates a new enrichment entry from a classified value, deferring
    /// redaction to emission time.
    ///
    /// The value is stored as a trait object and redacted through
    /// [`RedactedDisplay`](data_privacy::RedactedDisplay) when the enrichment
    /// is resolved by a processor. This means classified newtypes
    /// (`#[classified(...)]`) do **not** need an `Into<Value>` impl.
    pub fn new(key: impl Into<Key>, value: impl data_privacy::RedactedDisplay + Send + Sync + 'static) -> Self {
        Self {
            key: key.into(),
            stored: UnredactedValue::Unredacted(Arc::new(value)),
            exclude_from_logs: false,
            metric_key: None,
            target: None,
        }
    }

    /// Creates a new enrichment entry for an unclassified primitive value.
    ///
    /// Primitive values (`i64`, `f64`, `bool`, etc.) do not carry data
    /// classification. They are stored directly without redaction.
    pub fn unclassified(key: impl Into<Key>, value: impl Into<Value>) -> Self {
        let value: Value = value.into();
        Self {
            key: key.into(),
            stored: UnredactedValue::Primitive(value),
            exclude_from_logs: false,
            metric_key: None,
            target: None,
        }
    }

    /// Excludes this enrichment from log records.
    #[must_use]
    pub fn exclude_from_logs(mut self) -> Self {
        self.exclude_from_logs = true;
        self
    }

    /// Includes this enrichment as a metric dimension under the given key.
    ///
    /// Like event fields, enrichment is **not** a metric dimension by default;
    /// this is the explicit opt-in emitted by `#[dimension]`.
    #[must_use]
    pub fn with_metric_dimension(mut self, key: impl Into<Key>) -> Self {
        self.metric_key = Some(key.into());
        self
    }

    /// Sets the target sink for this enrichment entry.
    #[must_use]
    pub fn with_target(mut self, target: SinkId) -> Self {
        self.target = Some(target);
        self
    }

    /// Returns the target sink id, if this is a targeted enrichment.
    #[must_use]
    pub fn target(&self) -> Option<SinkId> {
        self.target
    }

    /// Returns `true` if this is a global enrichment (no target).
    #[must_use]
    pub fn is_global(&self) -> bool {
        self.target.is_none()
    }

    /// Returns a reference to the enrichment key.
    #[must_use]
    pub fn key(&self) -> &Key {
        &self.key
    }

    /// Returns `true` if this enrichment is excluded from log records.
    #[must_use]
    pub fn is_excluded_from_logs(&self) -> bool {
        self.exclude_from_logs
    }

    /// Returns the metric dimension key, if this enrichment opts into metrics.
    #[must_use]
    pub fn metric_key(&self) -> Option<&Key> {
        self.metric_key.as_ref()
    }

    /// Returns the value, applying redaction through the engine.
    ///
    /// This is gated behind the `test-util` feature for test assertions.
    #[cfg(any(test, feature = "test-util"))]
    #[must_use]
    pub fn redacted_value(&self, engine: &data_privacy::RedactionEngine) -> Value {
        self.redacted_value_inner(engine)
    }

    /// Returns the value, applying redaction through the engine.
    pub(crate) fn redacted_value_inner(&self, engine: &data_privacy::RedactionEngine) -> Value {
        match &self.stored {
            UnredactedValue::Primitive(value) => value.clone(),
            UnredactedValue::Unredacted(classified) => Value::from_redacted(&**classified, engine),
        }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_global_reflects_target() {
        let global = EnrichmentEntry::unclassified("k", 1_i64);
        assert!(global.is_global());

        let targeted = EnrichmentEntry::unclassified("k", 1_i64).with_target(SinkId::new("s"));
        assert!(!targeted.is_global());
    }
}
