// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Live counter deltas, separate from manually captured or filtered event data.

use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

use seismograph_protocol::message::{EventClassCounts, RecorderActivity, RecorderStatistics, ThreadRecorderStatistics};

pub(super) const CLASS_LABELS: [&str; 6] = ["Allocations", "General", "Arc deref", "Runtime", "I/O", "Cache"];
const HISTORY_LENGTH: usize = 120;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum RecorderUpdate {
    Detailed(RecorderActivity),
    Legacy(RecorderStatistics),
}

impl RecorderUpdate {
    pub(super) const fn statistics(&self) -> RecorderStatistics {
        match self {
            Self::Detailed(activity) => activity.statistics,
            Self::Legacy(statistics) => *statistics,
        }
    }
}

impl From<RecorderStatistics> for RecorderUpdate {
    fn from(statistics: RecorderStatistics) -> Self {
        Self::Legacy(statistics)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) enum Availability {
    #[default]
    Waiting,
    Supported,
    Unsupported,
}

#[derive(Clone, Debug)]
pub(super) struct ClassRateSample {
    pub(super) captured_at: Instant,
    pub(super) rates: [u64; 6],
}

#[derive(Clone, Debug)]
pub(super) struct ThreadActivity {
    pub(super) statistics: ThreadRecorderStatistics,
    pub(super) rate: Option<u64>,
    pub(super) history: VecDeque<u64>,
}

#[derive(Default)]
pub(super) struct LiveActivity {
    pub(super) availability: Availability,
    pub(super) stale: bool,
    pub(super) samples: VecDeque<ClassRateSample>,
    pub(super) threads: Vec<ThreadActivity>,
    previous: Option<(u64, [u64; 6], Instant)>,
}

impl LiveActivity {
    /// Returns whether the counter epoch changed, invalidating aggregate rate baselines too.
    pub(super) fn record(&mut self, activity: RecorderActivity, now: Instant) -> bool {
        let counts = class_counts(activity.class_events);
        let reset = self.previous.is_some_and(|(session, previous, _)| {
            session != activity.session_id || counts.iter().zip(previous).any(|(current, previous)| *current < previous)
        });
        if reset {
            self.samples.clear();
            self.threads.clear();
            self.previous = None;
        }
        let elapsed = self.previous.and_then(|(_, _, observed)| now.checked_duration_since(observed));
        if let Some((_, previous, _)) = self.previous
            && let Some(elapsed) = elapsed.filter(|duration| !duration.is_zero())
            && let Some(rates) = class_rates(previous, counts, elapsed)
        {
            self.samples.push_back(ClassRateSample { captured_at: now, rates });
            drop(self.samples.drain(..self.samples.len().saturating_sub(HISTORY_LENGTH)));
        }
        let mut previous = std::mem::take(&mut self.threads)
            .into_iter()
            .map(|thread| (thread.statistics.thread_id, thread))
            .collect::<BTreeMap<_, _>>();
        self.threads = activity
            .threads
            .into_iter()
            .map(|statistics| {
                let before = previous.remove(&statistics.thread_id);
                let current_rate = before
                    .as_ref()
                    .and_then(|before| rate(before.statistics.total_events, statistics.total_events, elapsed?));
                let mut history = before.map_or_else(VecDeque::new, |thread| thread.history);
                if let Some(rate) = current_rate {
                    history.push_back(rate);
                    drop(history.drain(..history.len().saturating_sub(HISTORY_LENGTH)));
                } else {
                    history.clear();
                }
                ThreadActivity {
                    statistics,
                    rate: current_rate,
                    history,
                }
            })
            .collect();
        self.threads.sort_unstable_by_key(|thread| thread.statistics.thread_id);
        self.previous = Some((activity.session_id, counts, now));
        self.availability = Availability::Supported;
        self.stale = false;
        reset
    }

    pub(super) fn unsupported(&mut self) {
        *self = Self {
            availability: Availability::Unsupported,
            ..Self::default()
        };
    }
}

const fn class_counts(counts: EventClassCounts) -> [u64; 6] {
    [
        counts.allocations,
        counts.general_events,
        counts.arc_dereferences,
        counts.runtime_tasks,
        counts.io,
        counts.cache,
    ]
}

fn rate(previous: u64, current: u64, elapsed: Duration) -> Option<u64> {
    let difference = current.checked_sub(previous)?;
    let per_second = u128::from(difference).checked_mul(1_000_000_000)?.checked_div(elapsed.as_nanos())?;
    Some(u64::try_from(per_second).unwrap_or(u64::MAX))
}

fn class_rates(previous: [u64; 6], current: [u64; 6], elapsed: Duration) -> Option<[u64; 6]> {
    let mut rates = [0; 6];
    for index in 0..rates.len() {
        rates[index] = rate(previous[index], current[index], elapsed)?;
    }
    Some(rates)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn activity(session_id: u64, counts: [u64; 6], threads: &[(u64, u64)]) -> RecorderActivity {
        RecorderActivity {
            session_id,
            class_events: EventClassCounts {
                allocations: counts[0],
                general_events: counts[1],
                arc_dereferences: counts[2],
                runtime_tasks: counts[3],
                io: counts[4],
                cache: counts[5],
            },
            threads: threads
                .iter()
                .map(|&(thread_id, total_events)| ThreadRecorderStatistics {
                    thread_id,
                    total_events,
                    retained_events: total_events.min(64),
                    event_capacity: 64,
                    ..ThreadRecorderStatistics::default()
                })
                .collect(),
            ..RecorderActivity::default()
        }
    }

    #[test]
    fn class_rates_use_elapsed_time_and_thread_rates_use_identity_not_row_position() {
        let mut live = LiveActivity::default();
        let start = Instant::now();
        live.record(activity(1, [10, 20, 30, 40, 50, 60], &[(7, 100), (2, 5)]), start);
        assert!(live.samples.is_empty());
        assert!(live.threads.iter().all(|thread| thread.rate.is_none()));
        live.record(
            activity(1, [12, 24, 36, 48, 60, 72], &[(9, 80), (2, 9), (7, 112)]),
            start + Duration::from_secs(2),
        );
        assert_eq!(live.samples.back().unwrap().rates, [1, 2, 3, 4, 5, 6]);
        assert_eq!(
            live.threads
                .iter()
                .map(|thread| (thread.statistics.thread_id, thread.rate))
                .collect::<Vec<_>>(),
            [(2, Some(2)), (7, Some(6)), (9, None)]
        );
    }

    #[test]
    fn new_epochs_and_regressed_counters_do_not_spike_even_if_totals_overtake_old_values() {
        let mut live = LiveActivity::default();
        let start = Instant::now();
        live.record(activity(1, [10; 6], &[(1, 10)]), start);
        assert!(live.record(activity(2, [100; 6], &[(1, 100)]), start + Duration::from_secs(1)));
        assert!(live.samples.is_empty());
        assert_eq!(live.threads[0].rate, None);
        assert!(live.record(activity(2, [1; 6], &[(1, 1)]), start + Duration::from_secs(2)));
        assert!(live.samples.is_empty());
        live.record(activity(2, [1; 6], &[(1, 1)]), start + Duration::from_secs(3));
        assert_eq!((live.samples.back().unwrap().rates, live.threads[0].rate), ([0; 6], Some(0)));
    }

    #[test]
    fn retired_threads_keep_rates_without_claiming_retained_events_after_ring_release() {
        let mut live = LiveActivity::default();
        let start = Instant::now();
        live.record(activity(1, [100; 6], &[(1, 100), (2, 100)]), start);
        let mut after = activity(1, [102; 6], &[(1, 100), (2, 112)]);
        after.threads[0].retired = true;
        after.threads[0].event_capacity = 0;
        after.threads[0].retained_events = 0;
        assert!(!live.record(after, start + Duration::from_secs(1)));
        assert_eq!(live.samples.back().unwrap().rates, [2; 6]);
        assert_eq!(
            live.threads.iter().map(|thread| thread.rate).collect::<Vec<_>>(),
            [Some(0), Some(12)]
        );
        assert_eq!(
            (
                live.threads[0].statistics.event_capacity,
                live.threads[0].statistics.retained_events
            ),
            (0, 0)
        );
    }

    #[test]
    fn rate_handles_zero_intervals_resets_large_counts_and_real_zeroes() {
        assert_eq!(
            [
                rate(0, 1, Duration::ZERO),
                rate(2, 1, Duration::from_secs(1)),
                rate(1, 1, Duration::from_secs(1)),
                rate(0, u64::MAX, Duration::from_nanos(1))
            ],
            [None, None, Some(0), Some(u64::MAX)],
        );
    }

    #[test]
    fn histories_are_bounded_and_unavailable_servers_do_not_report_empty_recording() {
        let mut live = LiveActivity::default();
        let start = Instant::now();
        for count in 0..130 {
            live.record(activity(1, [count; 6], &[(1, count)]), start + Duration::from_secs(count));
        }
        assert_eq!(
            (live.samples.len(), live.threads[0].history.len()),
            (HISTORY_LENGTH, HISTORY_LENGTH)
        );
        live.stale = true;
        live.unsupported();
        assert_eq!(live.availability, Availability::Unsupported);
        assert!(live.samples.is_empty() && live.threads.is_empty());
        assert!(!live.stale);
    }
}
