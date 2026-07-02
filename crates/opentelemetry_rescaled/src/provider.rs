// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The [`RescaledMetrics`] meter provider and its builder.

use std::any::type_name;
use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use foldhash::fast::RandomState;
use opentelemetry::InstrumentationScope;
use opentelemetry::metrics::{Meter, MeterProvider};

use crate::ScopeConfigurator;
use crate::config::ScopeRules;
use crate::instruments::RescalingInstrumentProvider;

/// A meter provider that wraps an inner provider and emits rescaled side-by-side copies of selected instruments.
///
/// Build one with [`RescaledMetrics::builder`]. The result is itself a
/// [`MeterProvider`], so it can be handed wherever the inner provider went —
/// including [`opentelemetry::global::set_meter_provider`].
///
/// Scopes that carry no rescale rules are returned untouched, so unconfigured
/// telemetry pays no wrapping cost.
#[derive(Clone)]
pub struct RescaledMetrics {
    inner: Arc<dyn MeterProvider + Send + Sync>,
    scopes: Arc<HashMap<Cow<'static, str>, Arc<ScopeRules>, RandomState>>,
}

impl RescaledMetrics {
    /// Starts building a [`RescaledMetrics`] wrapping `inner`.
    ///
    /// The inner provider is taken by value and type-erased, so `RescaledMetrics`
    /// carries no generic parameter for it.
    pub fn builder(inner: impl MeterProvider + Send + Sync + 'static) -> RescaledMetricsBuilder {
        RescaledMetricsBuilder {
            inner: Arc::new(inner),
            scopes: HashMap::default(),
        }
    }
}

impl fmt::Debug for RescaledMetrics {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("scopes", &self.scopes.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

impl MeterProvider for RescaledMetrics {
    fn meter_with_scope(&self, scope: InstrumentationScope) -> Meter {
        let inner_meter = self.inner.meter_with_scope(scope.clone());
        match self.scopes.get(scope.name()) {
            Some(rules) => Meter::new(Arc::new(RescalingInstrumentProvider::new(inner_meter, Arc::clone(rules)))),
            None => inner_meter,
        }
    }
}

/// Builder for [`RescaledMetrics`].
///
/// Register rescaling per scope with [`scope`](Self::scope), then finish with
/// [`build`](Self::build).
pub struct RescaledMetricsBuilder {
    inner: Arc<dyn MeterProvider + Send + Sync>,
    scopes: HashMap<Cow<'static, str>, ScopeConfigurator, RandomState>,
}

impl RescaledMetricsBuilder {
    /// Configures rescaling for the instrumentation scope named `name`.
    ///
    /// The `configure` closure receives a [`ScopeConfigurator`] on which to
    /// declare sidecars via [`ScopeConfigurator::rescale`]. Calling `scope`
    /// more than once with the same name accumulates rules into that scope.
    ///
    /// Scopes are matched by name only; if several instrumentation scopes share
    /// a name, the rules apply to all of them.
    #[must_use]
    pub fn scope(mut self, name: impl Into<Cow<'static, str>>, configure: impl FnOnce(&mut ScopeConfigurator)) -> Self {
        let configurator = self.scopes.entry(name.into()).or_default();
        configure(configurator);
        self
    }

    /// Builds the [`RescaledMetrics`] provider.
    ///
    /// Scopes for which no rules were declared are dropped, so they pass through
    /// to the inner provider with no wrapping.
    #[must_use]
    pub fn build(self) -> RescaledMetrics {
        let scopes = self
            .scopes
            .into_iter()
            .map(|(name, configurator)| (name, configurator.into_rules()))
            .filter(|(_, rules)| !rules.is_empty())
            .map(|(name, rules)| (name, Arc::new(rules)))
            .collect();

        RescaledMetrics {
            inner: self.inner,
            scopes: Arc::new(scopes),
        }
    }
}

impl fmt::Debug for RescaledMetricsBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("scopes", &self.scopes.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}
