// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Rescale configuration: the rules that map a source instrument, within a
//! scope, to one or more rescaled sidecars.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

/// A single rescaled sidecar of a source instrument.
#[derive(Debug, Clone)]
pub(crate) struct RescaleRule {
    /// Name of the sidecar instrument.
    pub(crate) target_name: Cow<'static, str>,
    /// Unit of the sidecar instrument (mandatory; rescaling changes the unit).
    pub(crate) target_unit: Cow<'static, str>,
    /// Multiplicative factor applied to each measurement.
    pub(crate) factor: f64,
}

/// The rescale rules for one instrumentation scope, keyed by source instrument name.
#[derive(Debug, Default)]
pub(crate) struct ScopeRules {
    map: HashMap<Cow<'static, str>, Vec<RescaleRule>>,
}

impl ScopeRules {
    /// Returns the rescale rules for an instrument name, if any are configured.
    pub(crate) fn rules_for(&self, name: &str) -> Option<&[RescaleRule]> {
        self.map.get(name).map(Vec::as_slice)
    }

    /// Returns `true` if no rules are configured for this scope.
    pub(crate) fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

/// Collects the rescale rules for a single instrumentation scope.
///
/// An instance is passed to the closure given to
/// [`RescaledMetricsBuilder::scope`](crate::RescaledMetricsBuilder::scope);
/// call [`rescale`](Self::rescale) on it to register sidecars.
#[derive(Debug, Default)]
pub struct ScopeConfigurator {
    rules: HashMap<Cow<'static, str>, Vec<RescaleRule>>,
    targets: HashSet<Cow<'static, str>>,
}

impl ScopeConfigurator {
    /// Registers a rescaled sidecar for the `source` instrument.
    ///
    /// Whenever the `source` instrument is created in this scope, a second
    /// instrument named `target` (carrying `unit`) is created alongside it,
    /// recording every measurement multiplied by `factor`.
    ///
    /// # Panics
    ///
    /// Panics if the configuration cannot produce a meaningful sidecar:
    /// - `factor` is not a finite, strictly positive number;
    /// - `source` and `target` are equal;
    /// - `target` is already used by another rule in this scope.
    pub fn rescale(
        &mut self,
        source: impl Into<Cow<'static, str>>,
        target: impl Into<Cow<'static, str>>,
        unit: impl Into<Cow<'static, str>>,
        factor: f64,
    ) -> &mut Self {
        let source = source.into();
        let target = target.into();
        let unit = unit.into();

        assert!(
            factor.is_finite() && factor > 0.0,
            "rescale factor must be a finite, strictly positive number, got {factor}"
        );
        assert!(source != target, "rescale source and target must differ, both are '{source}'");
        assert!(
            self.targets.insert(target.clone()),
            "duplicate rescale target '{target}' within a scope"
        );

        self.rules.entry(source).or_default().push(RescaleRule {
            target_name: target,
            target_unit: unit,
            factor,
        });
        self
    }

    /// Consumes the configurator, yielding the collected per-scope rules.
    pub(crate) fn into_rules(self) -> ScopeRules {
        ScopeRules { map: self.rules }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn collects_multiple_sidecars_per_source() {
        let mut sc = ScopeConfigurator::default();
        sc.rescale("dur", "dur.ms", "ms", 1000.0)
            .rescale("dur", "dur.us", "us", 1_000_000.0)
            .rescale("size", "size.kb", "kB", 0.001);

        let rules = sc.into_rules();
        assert_eq!(rules.rules_for("dur").expect("dur has rules").len(), 2);
        assert_eq!(rules.rules_for("size").expect("size has rules").len(), 1);
        assert!(rules.rules_for("missing").is_none());
        assert!(!rules.is_empty());
    }

    #[test]
    fn empty_configurator_is_empty() {
        let rules = ScopeConfigurator::default().into_rules();
        assert!(rules.is_empty());
    }

    #[test]
    #[should_panic(expected = "finite, strictly positive")]
    fn rejects_zero_factor() {
        ScopeConfigurator::default().rescale("a", "b", "u", 0.0);
    }

    #[test]
    #[should_panic(expected = "finite, strictly positive")]
    fn rejects_negative_factor() {
        ScopeConfigurator::default().rescale("a", "b", "u", -1.0);
    }

    #[test]
    #[should_panic(expected = "finite, strictly positive")]
    fn rejects_nan_factor() {
        ScopeConfigurator::default().rescale("a", "b", "u", f64::NAN);
    }

    #[test]
    #[should_panic(expected = "finite, strictly positive")]
    fn rejects_infinite_factor() {
        ScopeConfigurator::default().rescale("a", "b", "u", f64::INFINITY);
    }

    #[test]
    #[should_panic(expected = "source and target must differ")]
    fn rejects_source_equal_target() {
        ScopeConfigurator::default().rescale("same", "same", "u", 2.0);
    }

    #[test]
    #[should_panic(expected = "duplicate rescale target")]
    fn rejects_duplicate_target() {
        ScopeConfigurator::default()
            .rescale("a", "shared", "u", 2.0)
            .rescale("b", "shared", "u", 3.0);
    }
}
