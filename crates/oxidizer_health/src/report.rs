// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::{self, Display, Formatter};
use std::iter::FusedIterator;
use std::sync::Arc;
use std::{slice, vec};

use bitflags::bitflags;

use crate::{Health, Signal};

bitflags! {
    /// Selects which publisher signals a [`Report`] retains.
    ///
    /// Publisher counts remain available for every state, even when its signals are filtered out.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxidizer_health::Filter;
    ///
    /// let problems = Filter::DEGRADED | Filter::UNHEALTHY;
    /// assert!(!problems.contains(Filter::HEALTHY));
    /// assert!(Filter::ALL.contains(problems));
    /// ```
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub struct Filter: u8 {
        /// Retains `Healthy` signals.
        const HEALTHY = 1;
        /// Retains `Degraded` signals.
        const DEGRADED = 1 << 1;
        /// Retains `Unhealthy` signals.
        const UNHEALTHY = 1 << 2;
        /// Retains all signals.
        const ALL = Self::HEALTHY.bits() | Self::DEGRADED.bits() | Self::UNHEALTHY.bits();
    }
}

impl Filter {
    const fn selects(self, state: Health) -> bool {
        let flag = match state {
            Health::Healthy => Self::HEALTHY,
            Health::Degraded => Self::DEGRADED,
            Health::Unhealthy => Self::UNHEALTHY,
        };
        self.contains(flag)
    }
}

/// Immutable aggregation result of one component, shared by its reports.
#[derive(Debug, Default)]
pub(crate) struct Snapshot {
    pub(crate) counts: [usize; Health::COUNT],
    pub(crate) signals: [Vec<(Arc<Signal>, usize)>; Health::COUNT],
}

impl Snapshot {
    /// Groups publisher signals into distinct signals with publisher counts.
    pub(crate) fn collect<'a>(signals: impl IntoIterator<Item = &'a Arc<Signal>>) -> Self {
        let mut snapshot = Self::default();
        for signal in signals {
            let index = signal.state().index();
            snapshot.counts[index] += 1;
            let distinct = &mut snapshot.signals[index];
            match distinct.iter_mut().find(|(existing, _)| **existing == **signal) {
                Some((_, count)) => *count += 1,
                None => distinct.push((Arc::clone(signal), 1)),
            }
        }
        snapshot
    }

    pub(crate) fn state(&self) -> Health {
        Health::worst(&self.counts)
    }
}

/// Immutable snapshot of one component.
///
/// A report contains the component name, its aggregate state, publisher counts by state, and the
/// distinct signals selected by a [`Filter`] with the number of publishers reporting each one.
///
/// # Examples
///
/// ```
/// # testing_aids::init_tracing!();
/// use oxidizer_health::{Aggregator, Filter, Health};
///
/// # futures::executor::block_on(async {
/// let aggregator = Aggregator::new();
/// let mut database = aggregator.component("database");
/// let mut primary = database.publisher();
/// let _replica = database.publisher();
/// primary.degraded("high latency");
///
/// let report = loop {
///     let report = database.report(Filter::DEGRADED).await;
///     if report.signal_count(Health::Healthy) == 1 && report.signal_count(Health::Degraded) == 1 {
///         break report;
///     }
///     database.changed().await;
/// };
/// assert_eq!(report.name(), "database");
/// assert_eq!(report.state(), Health::Degraded);
/// assert_eq!(report.signal_count(Health::Healthy), 1);
/// assert_eq!(report.signals(Health::Healthy).len(), 0);
///
/// let (signal, publishers) = report.signals(Health::Degraded).next().unwrap();
/// assert_eq!(signal.to_string(), "Degraded (reason=high latency)");
/// assert_eq!(publishers, 1);
/// # });
/// ```
#[derive(Clone, Debug)]
pub struct Report {
    name: Arc<str>,
    state: Health,
    counts: [usize; Health::COUNT],
    snapshot: Arc<Snapshot>,
    filter: Filter,
}

impl Report {
    pub(crate) fn new(name: Arc<str>, snapshot: Arc<Snapshot>, filter: Filter) -> Self {
        Self {
            name,
            state: snapshot.state(),
            counts: snapshot.counts,
            snapshot,
            filter,
        }
    }

    /// Returns the component name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the aggregate component state.
    #[must_use]
    pub const fn state(&self) -> Health {
        self.state
    }

    /// Returns the number of publishers in `state`.
    #[must_use]
    pub const fn signal_count(&self, state: Health) -> usize {
        self.counts[state.index()]
    }

    /// Returns the distinct signals in `state` with their publisher counts.
    ///
    /// The iterator is empty when the report filter excludes `state`.
    #[must_use]
    pub fn signals(&self, state: Health) -> Signals<'_> {
        let signals: &[_] = if self.filter.selects(state) {
            &self.snapshot.signals[state.index()]
        } else {
            &[]
        };
        Signals { inner: signals.iter() }
    }
}

impl Display for Report {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}: {} (healthy: {}, degraded: {}, unhealthy: {})",
            self.name,
            self.state,
            self.signal_count(Health::Healthy),
            self.signal_count(Health::Degraded),
            self.signal_count(Health::Unhealthy),
        )
    }
}

/// Iterator over component [`Report`]s, in unspecified order.
#[derive(Clone, Debug)]
pub struct Reports {
    inner: vec::IntoIter<Report>,
}

impl Reports {
    pub(crate) fn new(reports: Vec<Report>) -> Self {
        Self {
            inner: reports.into_iter(),
        }
    }
}

impl Iterator for Reports {
    type Item = Report;

    fn next(&mut self) -> Option<Report> {
        self.inner.next()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl ExactSizeIterator for Reports {}

impl FusedIterator for Reports {}

/// Iterator over distinct signals in a [`Report`] and the number of publishers reporting each.
#[derive(Clone, Debug)]
pub struct Signals<'a> {
    inner: slice::Iter<'a, (Arc<Signal>, usize)>,
}

impl Iterator for Signals<'_> {
    type Item = (Signal, usize);

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next().map(|(signal, count)| (Signal::clone(signal), *count))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl ExactSizeIterator for Signals<'_> {}

impl FusedIterator for Signals<'_> {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Attribute;

    fn signal(state: Health, reason: &str) -> Arc<Signal> {
        Arc::new(Signal::new(state, Box::new([Attribute::from(("reason", reason))])))
    }

    #[test]
    fn collect_groups_equal_signals() {
        let slow = signal(Health::Degraded, "slow");
        let signals = [
            Arc::clone(&slow),
            signal(Health::Degraded, "slow"),
            signal(Health::Degraded, "busy"),
        ];

        let snapshot = Snapshot::collect(&signals);

        assert_eq!(snapshot.counts, [0, 3, 0]);
        assert_eq!(snapshot.signals[Health::Degraded.index()].len(), 2);
        assert_eq!(snapshot.state(), Health::Degraded);
    }

    #[test]
    fn empty_snapshot_is_unhealthy() {
        assert_eq!(Snapshot::collect([]).state(), Health::Unhealthy);
    }

    #[test]
    fn filter_hides_signals_but_keeps_counts() {
        let signals = [signal(Health::Unhealthy, "down"), signal(Health::Healthy, "ok")];
        let report = Report::new(Arc::from("db"), Arc::new(Snapshot::collect(&signals)), Filter::UNHEALTHY);

        assert_eq!(report.signal_count(Health::Healthy), 1);
        assert_eq!(report.signals(Health::Healthy).count(), 0);
        assert_eq!(report.signals(Health::Unhealthy).len(), 1);
        assert_eq!(report.to_string(), "db: Unhealthy (healthy: 1, degraded: 0, unhealthy: 1)");
    }
}
