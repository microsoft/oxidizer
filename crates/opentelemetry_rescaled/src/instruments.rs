// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The rescaling [`InstrumentProvider`]: builds each configured source instrument together with its rescaled sidecars.

use std::any::type_name;
use std::borrow::Cow;
use std::fmt;
use std::marker::PhantomData;
use std::sync::Arc;

use opentelemetry::KeyValue;
use opentelemetry::metrics::{
    AsyncInstrument, AsyncInstrumentBuilder, Callback, Counter, Gauge, Histogram, HistogramBuilder, InstrumentBuilder, InstrumentProvider,
    Meter, ObservableCounter, ObservableGauge, ObservableUpDownCounter, SyncInstrument, UpDownCounter,
};

use crate::config::ScopeRules;
use crate::rescale::Rescale;

/// An [`InstrumentProvider`] that mirrors configured instruments into rescaled sidecars, delegating all real work to the wrapped inner [`Meter`].
pub(crate) struct RescalingInstrumentProvider {
    inner: Meter,
    rules: Arc<ScopeRules>,
}

impl RescalingInstrumentProvider {
    pub(crate) fn new(inner: Meter, rules: Arc<ScopeRules>) -> Self {
        Self { inner, rules }
    }
}

impl fmt::Debug for RescalingInstrumentProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>()).finish_non_exhaustive()
    }
}

/// A built synchronous instrument that records a single measurement.
///
/// Unifies the differently named recording methods (`add`/`record`) so the
/// [`FanOut`] can drive any synchronous instrument uniformly.
pub(crate) trait Record<T> {
    fn record_value(&self, value: T, attributes: &[KeyValue]);
}

impl<T> Record<T> for Counter<T> {
    fn record_value(&self, value: T, attributes: &[KeyValue]) {
        self.add(value, attributes);
    }
}

impl<T> Record<T> for UpDownCounter<T> {
    fn record_value(&self, value: T, attributes: &[KeyValue]) {
        self.add(value, attributes);
    }
}

impl<T> Record<T> for Gauge<T> {
    fn record_value(&self, value: T, attributes: &[KeyValue]) {
        self.record(value, attributes);
    }
}

impl<T> Record<T> for Histogram<T> {
    fn record_value(&self, value: T, attributes: &[KeyValue]) {
        self.record(value, attributes);
    }
}

/// A synchronous instrument backing that fans each measurement out to the original instrument and every rescaled sidecar.
struct FanOut<T, R> {
    original: R,
    sidecars: Vec<(R, f64)>,
    _value: PhantomData<fn(T)>,
}

impl<T, R> SyncInstrument<T> for FanOut<T, R>
where
    T: Rescale,
    R: Record<T> + Send + Sync,
{
    fn measure(&self, measurement: T, attributes: &[KeyValue]) {
        self.original.record_value(measurement, attributes);
        for (sidecar, factor) in &self.sidecars {
            sidecar.record_value(measurement.rescale(*factor), attributes);
        }
    }
}

/// An observer that scales every observation before forwarding it to the sidecar's inner observer.
struct ScalingObserver<'a, M> {
    inner: &'a dyn AsyncInstrument<M>,
    factor: f64,
}

impl<M> AsyncInstrument<M> for ScalingObserver<'_, M>
where
    M: Rescale,
{
    fn observe(&self, measurement: M, attributes: &[KeyValue]) {
        self.inner.observe(measurement.rescale(self.factor), attributes);
    }
}

impl RescalingInstrumentProvider {
    /// Builds a synchronous instrument, returning either the plain inner instrument (no rules) or one whose backing is a [`FanOut`] over the original plus every configured sidecar.
    fn make_sync<T, Inst>(
        &self,
        name: Cow<'static, str>,
        unit: Option<Cow<'static, str>>,
        build_one: impl Fn(Cow<'static, str>, Option<Cow<'static, str>>) -> Inst,
        wrap: impl FnOnce(Arc<dyn SyncInstrument<T> + Send + Sync>) -> Inst,
    ) -> Inst
    where
        T: Rescale,
        Inst: Record<T> + Send + Sync + 'static,
    {
        let Some(rules) = self.rules.rules_for(&name) else {
            return build_one(name, unit);
        };

        let sidecars = rules
            .iter()
            .map(|rule| {
                let sidecar = build_one(rule.target_name.clone(), Some(rule.target_unit.clone()));
                (sidecar, rule.factor)
            })
            .collect();

        let fan_out: Arc<dyn SyncInstrument<T> + Send + Sync> = Arc::new(FanOut {
            original: build_one(name, unit),
            sidecars,
            _value: PhantomData,
        });
        wrap(fan_out)
    }
}

/// Reconstructs a synchronous instrument builder on the inner meter, applying the inherited description and the given unit, then builds it.
macro_rules! sync_method {
    ($method:ident, $inst:ident, $value:ty) => {
        fn $method(&self, builder: InstrumentBuilder<'_, $inst<$value>>) -> $inst<$value> {
            let description = builder.description;
            self.make_sync(
                builder.name,
                builder.unit,
                |name, unit| {
                    let mut inner = self.inner.$method(name);
                    if let Some(description) = description.clone() {
                        inner = inner.with_description(description);
                    }
                    if let Some(unit) = unit {
                        inner = inner.with_unit(unit);
                    }
                    inner.build()
                },
                $inst::new,
            )
        }
    };
}

/// Reconstructs a histogram builder on the inner meter, scaling each sidecar's explicit bucket boundaries by its factor so the buckets stay meaningful.
macro_rules! histogram_method {
    ($method:ident, $value:ty) => {
        fn $method(&self, builder: HistogramBuilder<'_, Histogram<$value>>) -> Histogram<$value> {
            let description = builder.description;
            let source_unit = builder.unit;
            let boundaries = builder.boundaries;
            let name = builder.name;

            let build_one = |name: Cow<'static, str>, unit: Option<Cow<'static, str>>, boundaries: Option<Vec<f64>>| {
                let mut inner = self.inner.$method(name);
                if let Some(description) = description.clone() {
                    inner = inner.with_description(description);
                }
                if let Some(unit) = unit {
                    inner = inner.with_unit(unit);
                }
                if let Some(boundaries) = boundaries {
                    inner = inner.with_boundaries(boundaries);
                }
                inner.build()
            };

            let original = build_one(name.clone(), source_unit.clone(), boundaries.clone());
            let Some(rules) = self.rules.rules_for(&name) else {
                return original;
            };

            let sidecars = rules
                .iter()
                .map(|rule| {
                    let scaled_boundaries = boundaries
                        .as_ref()
                        .map(|bounds| bounds.iter().map(|bound| bound * rule.factor).collect());
                    let sidecar = build_one(rule.target_name.clone(), Some(rule.target_unit.clone()), scaled_boundaries);
                    (sidecar, rule.factor)
                })
                .collect();

            Histogram::new(Arc::new(FanOut {
                original,
                sidecars,
                _value: PhantomData,
            }))
        }
    };
}

/// Reconstructs an observable instrument on the inner meter, sharing the user callbacks between the original (identity) registration and one per sidecar (through a [`ScalingObserver`]).
///
/// Because each registration is independent, the callbacks run once per
/// registered instrument per collection.
macro_rules! observable_method {
    ($method:ident, $inst:ident, $value:ty) => {
        fn $method(&self, builder: AsyncInstrumentBuilder<'_, $inst<$value>, $value>) -> $inst<$value> {
            let name = builder.name;
            let description = builder.description;
            let unit = builder.unit;
            let callbacks: Arc<[Callback<$value>]> = builder.callbacks.into();

            {
                let callbacks = Arc::clone(&callbacks);
                let mut inner = self.inner.$method(name.clone());
                if let Some(description) = description.clone() {
                    inner = inner.with_description(description);
                }
                if let Some(unit) = unit.clone() {
                    inner = inner.with_unit(unit);
                }
                let _instrument = inner
                    .with_callback(move |observer| {
                        for callback in callbacks.iter() {
                            callback(observer);
                        }
                    })
                    .build();
            }

            if let Some(rules) = self.rules.rules_for(&name) {
                for rule in rules {
                    let callbacks = Arc::clone(&callbacks);
                    let factor = rule.factor;
                    let mut inner = self.inner.$method(rule.target_name.clone());
                    if let Some(description) = description.clone() {
                        inner = inner.with_description(description);
                    }
                    inner = inner.with_unit(rule.target_unit.clone());
                    let _instrument = inner
                        .with_callback(move |observer| {
                            let scaling = ScalingObserver { inner: observer, factor };
                            for callback in callbacks.iter() {
                                callback(&scaling);
                            }
                        })
                        .build();
                }
            }

            $inst::new()
        }
    };
}

impl InstrumentProvider for RescalingInstrumentProvider {
    sync_method!(u64_counter, Counter, u64);
    sync_method!(f64_counter, Counter, f64);
    sync_method!(i64_up_down_counter, UpDownCounter, i64);
    sync_method!(f64_up_down_counter, UpDownCounter, f64);
    sync_method!(u64_gauge, Gauge, u64);
    sync_method!(i64_gauge, Gauge, i64);
    sync_method!(f64_gauge, Gauge, f64);

    histogram_method!(u64_histogram, u64);
    histogram_method!(f64_histogram, f64);

    observable_method!(u64_observable_counter, ObservableCounter, u64);
    observable_method!(f64_observable_counter, ObservableCounter, f64);
    observable_method!(i64_observable_up_down_counter, ObservableUpDownCounter, i64);
    observable_method!(f64_observable_up_down_counter, ObservableUpDownCounter, f64);
    observable_method!(u64_observable_gauge, ObservableGauge, u64);
    observable_method!(i64_observable_gauge, ObservableGauge, i64);
    observable_method!(f64_observable_gauge, ObservableGauge, f64);
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use opentelemetry::metrics::MeterProvider as _;
    use opentelemetry::metrics::noop::NoopMeterProvider;

    use super::*;

    #[test]
    fn debug_is_non_exhaustive() {
        let meter = NoopMeterProvider::new().meter("scope");
        let provider = RescalingInstrumentProvider::new(meter, Arc::new(ScopeRules::default()));
        let rendered = format!("{provider:?}");
        assert!(rendered.contains("RescalingInstrumentProvider"));
        assert!(rendered.contains(".."));
    }
}
