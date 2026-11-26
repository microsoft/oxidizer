// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::type_name;
use std::fmt::{self, Debug, Display, Formatter};
use std::iter::FusedIterator;
use std::sync::{Arc, OnceLock};

use crate::{Health, Signal};

/// Immutable aggregation result of one component, shared by its reports.
#[derive(Debug, Default)]
pub(crate) struct Snapshot {
    signals: [SignalBucket; Health::COUNT],
}

#[derive(Debug, Default)]
struct SignalBucket {
    publishers: Vec<Arc<Signal>>,
    groups: OnceLock<Vec<(Arc<Signal>, usize)>>,
}

impl SignalBucket {
    fn groups(&self) -> &[(Arc<Signal>, usize)] {
        self.groups.get_or_init(|| {
            let mut groups: Vec<(Arc<Signal>, usize)> = Vec::new();
            for signal in &self.publishers {
                match groups.iter_mut().find(|(existing, _)| **existing == **signal) {
                    Some((_, count)) => *count += 1,
                    None => groups.push((Arc::clone(signal), 1)),
                }
            }
            groups
        })
    }
}

impl Snapshot {
    /// Capture the publishers without comparing diagnostics on the shared aggregation worker.
    pub(crate) fn collect<'a>(signals: impl IntoIterator<Item = &'a Arc<Signal>>) -> Self {
        let mut snapshot = Self::default();
        for signal in signals {
            snapshot.signals[signal.state().index()].publishers.push(Arc::clone(signal));
        }
        snapshot
    }

    fn counts(&self) -> [usize; Health::COUNT] {
        self.signals.each_ref().map(|bucket| bucket.publishers.len())
    }

    pub(crate) fn state(&self) -> Health {
        Health::worst(&self.counts())
    }
}

/// Immutable snapshot of one component.
///
/// A report contains the component name, its aggregate state, publisher counts by state, and the
/// distinct signals with the number of publishers reporting each one.
///
/// # Examples
///
/// ```
/// # testing_aids::init_tracing!();
/// use oxidizer_health::{Aggregator, Health};
///
/// # futures::executor::block_on(async {
/// let aggregator = Aggregator::new();
/// let mut database = aggregator.component("database");
/// let mut primary = database.publisher();
/// let _replica = database.publisher();
/// primary.degraded("high latency");
///
/// let report = loop {
///     let report = database.report().await;
///     if report.publisher_count(Health::Healthy) == 1
///         && report.publisher_count(Health::Degraded) == 1
///     {
///         break report;
///     }
///     database.changed().await;
/// };
/// assert_eq!(report.name(), "database");
/// assert_eq!(report.state(), Health::Degraded);
/// assert_eq!(report.publisher_count(Health::Healthy), 1);
/// assert_eq!(report.signals(Health::Healthy).len(), 1);
///
/// let (signal, publishers) = report.signals(Health::Degraded).next().unwrap();
/// assert_eq!(signal.to_string(), "Degraded (reason=high latency)");
/// assert_eq!(publishers, 1);
/// # });
/// ```
#[derive(Clone)]
pub struct Report {
    name: Arc<str>,
    state: Health,
    counts: [usize; Health::COUNT],
    snapshot: Arc<Snapshot>,
}

impl Report {
    pub(crate) fn new(name: Arc<str>, snapshot: Arc<Snapshot>) -> Self {
        Self {
            name,
            state: snapshot.state(),
            counts: snapshot.counts(),
            snapshot,
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
    pub const fn publisher_count(&self, state: Health) -> usize {
        self.counts[state.index()]
    }

    /// Returns the distinct signals in `state` with their publisher counts.
    #[must_use]
    pub fn signals(&self, state: Health) -> impl ExactSizeIterator<Item = (Signal, usize)> + FusedIterator + Clone + Debug + '_ {
        self.snapshot.signals[state.index()]
            .groups()
            .iter()
            .map(|(signal, count)| (Signal::clone(signal), *count))
    }
}

impl Debug for Report {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("name", &self.name)
            .field("state", &self.state)
            .field("counts", &self.counts)
            .finish_non_exhaustive()
    }
}

impl Display for Report {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}: {} (healthy: {}, degraded: {}, unhealthy: {})",
            self.name,
            self.state,
            self.publisher_count(Health::Healthy),
            self.publisher_count(Health::Degraded),
            self.publisher_count(Health::Unhealthy),
        )
    }
}

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
        let busy = signal(Health::Degraded, "busy");
        let signals = [Arc::clone(&slow), signal(Health::Degraded, "slow"), Arc::clone(&busy)];

        let snapshot = Snapshot::collect(&signals);

        let report = Report::new(Arc::from("clients"), Arc::new(snapshot));
        assert_eq!(report.state(), Health::Degraded);
        assert_eq!(report.publisher_count(Health::Degraded), 3);
        let groups: Vec<_> = report.signals(Health::Degraded).collect();
        assert_eq!(groups.len(), 2);
        assert!(groups.iter().any(|(signal, count)| signal == slow.as_ref() && *count == 2));
        assert!(groups.iter().any(|(signal, count)| signal == busy.as_ref() && *count == 1));
    }

    #[test]
    fn empty_snapshot_is_unhealthy() {
        assert_eq!(Snapshot::collect([]).state(), Health::Unhealthy);
    }

    #[test]
    fn report_keeps_all_signal_groups_and_counts() {
        let signals = [signal(Health::Unhealthy, "down"), signal(Health::Healthy, "ok")];
        let report = Report::new(Arc::from("db"), Arc::new(Snapshot::collect(&signals)));

        assert_eq!(report.publisher_count(Health::Healthy), 1);
        assert_eq!(report.signals(Health::Healthy).count(), 1);
        assert_eq!(report.signals(Health::Unhealthy).len(), 1);
    }

    #[test]
    fn display_summarizes_state_and_publisher_counts() {
        let signals = [signal(Health::Unhealthy, "down"), signal(Health::Healthy, "ok")];
        let report = Report::new(Arc::from("db"), Arc::new(Snapshot::collect(&signals)));

        assert_eq!(
            report.to_string(),
            "db: Unhealthy (healthy: 1, degraded: 0, unhealthy: 1)"
        );
    }
}
