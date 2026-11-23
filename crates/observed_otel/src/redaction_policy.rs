// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Fragment-retaining redaction recipe with overlay-merge support.
//!
//! [`RedactionPolicy`] holds the (`DataClass`, [`Redactor`]) entries used to
//! construct a fresh [`RedactionEngine`] per destination. Unlike an opaque
//! `RedactionEngine`, a `RedactionPolicy` can be **overlaid** onto another —
//! the [`Builder`](crate::Builder) merges the sink-level
//! baseline with each destination's overlay (set via
//! [`MetricDestinationExt`](crate::MetricDestinationExt::extend_redaction) and
//! [`LogDestinationExt`](crate::LogDestinationExt::extend_redaction)) and produces
//! one engine per destination. Overlay entries win on `DataClass` conflict.

use std::collections::HashMap;

use data_privacy::{DataClass, IntoDataClass, RedactionEngine, Redactor};
use performables::arc::Arc;

/// A reusable recipe for building a [`RedactionEngine`].
///
/// Construct with [`RedactionPolicy::new`] and add entries with
/// [`class`](Self::class). Pass to
/// [`Builder::redaction`](crate::Builder::redaction) for the
/// sink-level baseline, or to
/// [`MetricDestinationExt`](crate::MetricDestinationExt::extend_redaction) and
///  [`LogDestinationExt`](crate::LogDestinationExt::extend_redaction)
/// to overlay it on top of the baseline for one destination.
///
/// Cloning a policy is cheap — the inner redactor instances are shared via
/// `Arc`.
///
/// # Example
///
/// ```no_run
/// use data_privacy::DataClass;
/// use data_privacy::simple_redactor::SimpleRedactor;
/// use observed_otel::RedactionPolicy;
///
/// const PUBLIC: DataClass = DataClass::new("test", "public");
/// const DEBUG: DataClass = DataClass::new("test", "debug");
///
/// let baseline = RedactionPolicy::new().class(PUBLIC, SimpleRedactor::new());
///
/// let debug_overlay = RedactionPolicy::new().class(DEBUG, SimpleRedactor::new());
/// // pass `baseline` to the sink builder, `debug_overlay` to the destination
/// ```
#[derive(Clone, Default)]
pub struct RedactionPolicy {
    classes: HashMap<DataClass, Arc<dyn Redactor + Send + Sync>>,
}

impl RedactionPolicy {
    /// Creates an empty policy. With no entries, every `DataClass` falls
    /// through to the upstream default fallback redactor (erase).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a per-class redactor. Only one redactor can be active per
    /// `DataClass`: if `class` is called more than once with the same
    /// `DataClass` on a single policy, the last call wins. The same rule
    /// applies across layers — when this policy is later overlaid on top of
    /// another (or vice versa), entries added on the overlay win on
    /// `DataClass` conflict.
    #[must_use]
    pub fn class(mut self, data_class: impl IntoDataClass, redactor: impl Redactor + Send + Sync + 'static) -> Self {
        self.classes.insert(
            data_class.into_data_class(),
            Arc::new(redactor).coerce::<dyn Redactor + Send + Sync>(|arc| arc),
        );
        self
    }

    /// Constructs a fresh [`RedactionEngine`] from this policy's class
    /// entries. Use as an escape hatch when a caller needs a concrete
    /// engine; the framework's destination builders call this internally
    /// (with overlay merging) and don't require callers to invoke it.
    #[must_use]
    pub fn build(&self) -> RedactionEngine {
        engine_from_layers(self, None)
    }
}

impl std::fmt::Debug for RedactionPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct(std::any::type_name::<Self>())
            .field("classes", &self.classes.len())
            .finish_non_exhaustive()
    }
}

/// Lets a single `Arc<dyn Redactor>` be handed to multiple
/// `RedactionEngineBuilder` instances without duplicating redactor state.
struct ArcRedactor(Arc<dyn Redactor + Send + Sync>);

impl Redactor for ArcRedactor {
    fn redacts(&self, data_class: &DataClass) -> bool {
        self.0.redacts(data_class)
    }

    fn redact(&self, data_class: &DataClass, value: &str, output: &mut dyn std::fmt::Write) -> std::fmt::Result {
        self.0.redact(data_class, value, output)
    }
}

/// Builds a fresh [`RedactionEngine`] from a baseline policy plus an optional
/// overlay. Entries are deduplicated by `DataClass` here (overlay entries win
/// on conflict) before being registered with the upstream builder.
pub(crate) fn engine_from_layers(baseline: &RedactionPolicy, overlay: Option<&RedactionPolicy>) -> RedactionEngine {
    let mut merged = baseline.classes.clone();
    if let Some(overlay) = overlay {
        merged.extend(overlay.classes.iter().map(|(dc, redactor)| (dc.clone(), Arc::clone(redactor))));
    }
    let mut builder = RedactionEngine::builder();
    for (dc, redactor) in merged {
        builder = builder.add_class_redactor(dc, ArcRedactor(redactor));
    }
    builder.build()
}

#[cfg(test)]
mod tests {
    use data_privacy::simple_redactor::{SimpleRedactor, SimpleRedactorMode};

    use super::*;

    const DC_A: DataClass = DataClass::new("test", "a");
    const DC_B: DataClass = DataClass::new("test", "b");

    fn redact(engine: &RedactionEngine, dc: &DataClass, value: &str) -> String {
        let mut out = String::new();
        engine.redact(dc, value, &mut out).expect("redact ok");
        out
    }

    #[test]
    fn empty_policy_uses_default_fallback() {
        let policy = RedactionPolicy::new();
        let engine = policy.build();
        // Default fallback erases everything.
        assert_eq!(redact(&engine, &DC_A, "secret"), "");
    }

    #[test]
    fn debug_reports_configured_class_count() {
        let policy = RedactionPolicy::new().class(DC_A, SimpleRedactor::with_mode(SimpleRedactorMode::Passthrough));

        assert!(format!("{policy:?}").contains("classes: 1"));
    }

    #[test]
    fn class_entry_applies_to_matching_class() {
        let engine = RedactionPolicy::new()
            .class(DC_A, SimpleRedactor::with_mode(SimpleRedactorMode::Passthrough))
            .build();

        assert!(!engine.redacts(&DC_A));
        assert_eq!(redact(&engine, &DC_A, "secret"), "secret");
        assert_eq!(redact(&engine, &DC_B, "secret"), "");
    }

    #[test]
    fn class_entry_reports_when_values_are_redacted() {
        let engine = RedactionPolicy::new()
            .class(DC_A, SimpleRedactor::with_mode(SimpleRedactorMode::Erase))
            .build();

        assert!(engine.redacts(&DC_A));
    }

    #[test]
    fn overlay_entry_wins_on_conflict() {
        let baseline = RedactionPolicy::new().class(DC_A, SimpleRedactor::with_mode(SimpleRedactorMode::Erase));
        let overlay = RedactionPolicy::new().class(DC_A, SimpleRedactor::with_mode(SimpleRedactorMode::Passthrough));

        let merged = engine_from_layers(&baseline, Some(&overlay));
        assert_eq!(redact(&merged, &DC_A, "secret"), "secret");
    }

    #[test]
    fn overlay_entry_for_unrelated_class_does_not_disturb_baseline() {
        let baseline = RedactionPolicy::new().class(DC_A, SimpleRedactor::with_mode(SimpleRedactorMode::Passthrough));
        let overlay = RedactionPolicy::new().class(DC_B, SimpleRedactor::with_mode(SimpleRedactorMode::Erase));

        let merged = engine_from_layers(&baseline, Some(&overlay));
        // Baseline still applies to DC_A.
        assert_eq!(redact(&merged, &DC_A, "secret"), "secret");
        // Overlay applies to DC_B.
        assert_eq!(redact(&merged, &DC_B, "secret"), "");
    }

    #[test]
    fn duplicate_class_within_single_policy_keeps_last_write() {
        // Only one redactor is active per DataClass; the second `.class()`
        // call for DC_A overrides the first.
        let policy = RedactionPolicy::new()
            .class(DC_A, SimpleRedactor::with_mode(SimpleRedactorMode::Erase))
            .class(DC_A, SimpleRedactor::with_mode(SimpleRedactorMode::Passthrough));
        let engine = policy.build();
        assert_eq!(redact(&engine, &DC_A, "secret"), "secret");
    }

    #[test]
    fn no_overlay_yields_baseline_only() {
        let baseline = RedactionPolicy::new().class(DC_A, SimpleRedactor::with_mode(SimpleRedactorMode::Passthrough));
        let engine = engine_from_layers(&baseline, None);
        assert_eq!(redact(&engine, &DC_A, "secret"), "secret");
    }
}
