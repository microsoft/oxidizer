// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Retained execution evidence. Gaps are unobserved, never proof of an idle executor.

use std::sync::Arc;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct TimeWindow {
    pub(super) start: u64,
    pub(super) end: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Interval {
    pub(super) start: u64,
    pub(super) end: u64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct ExecutionMetrics {
    pub(super) poll_count: u64,
    pub(super) median_poll_nanos: Option<u64>,
    pub(super) max_poll_nanos: Option<u64>,
    pub(super) executing_fraction: Option<f64>,
    pub(super) polls: Vec<Interval>,
    pub(super) ready: Arc<[Interval]>,
    pub(super) poll_samples: Vec<u64>,
    pub(super) ready_samples: Arc<[u64]>,
    pub(super) wake_samples: Arc<[u64]>,
}

impl ExecutionMetrics {
    pub(super) fn finish(&mut self, window: Option<TimeWindow>) {
        self.poll_count = u64::try_from(self.poll_samples.len()).unwrap_or(u64::MAX);
        self.max_poll_nanos = self.poll_samples.iter().copied().max();
        self.median_poll_nanos = median_nanos(&mut self.poll_samples);
        self.polls = union(&self.polls);
        self.executing_fraction = window.and_then(|window| occupancy(&self.polls, window));
    }
}

pub(super) fn median_nanos(samples: &mut [u64]) -> Option<u64> {
    if samples.is_empty() {
        return None;
    }
    let middle = samples.len() / 2;
    let even = samples.len().is_multiple_of(2);
    let (lower, upper, _) = samples.select_nth_unstable(middle);
    if even {
        let lower = lower.iter().copied().max()?;
        Some(lower + (*upper - lower) / 2)
    } else {
        Some(*upper)
    }
}

pub(super) fn union(intervals: &[Interval]) -> Vec<Interval> {
    let mut ordered = intervals
        .iter()
        .copied()
        .filter(|interval| interval.start <= interval.end)
        .collect::<Vec<_>>();
    ordered.sort_unstable_by_key(|interval| (interval.start, interval.end));
    let mut merged: Vec<Interval> = Vec::new();
    for interval in ordered {
        if let Some(last) = merged.last_mut()
            && interval.start <= last.end
        {
            last.end = last.end.max(interval.end);
        } else {
            merged.push(interval);
        }
    }
    merged
}

/// The union is clipped before summing, so overlapping polls cannot exceed 100 percent.
pub(super) fn occupancy(intervals: &[Interval], window: TimeWindow) -> Option<f64> {
    let span = window.end.checked_sub(window.start).filter(|span| *span > 0)?;
    let clipped = intervals
        .iter()
        .filter_map(|interval| {
            let start = interval.start.max(window.start);
            let end = interval.end.min(window.end);
            (start < end || (interval.start == interval.end && start == end)).then_some(Interval { start, end })
        })
        .collect::<Vec<_>>();
    if clipped.is_empty() {
        return None;
    }
    let occupied = union(&clipped).iter().map(|interval| interval.end - interval.start).sum::<u64>();
    Some(std::time::Duration::from_nanos(occupied).as_secs_f64() / std::time::Duration::from_nanos(span).as_secs_f64())
}

/// Equal-width time bins; empty bins are unknown, including source-only legacy captures.
pub(super) fn bins(intervals: &[Interval], window: Option<TimeWindow>, count: usize) -> Vec<Option<f64>> {
    let Some(window) = window.filter(|window| window.end > window.start) else {
        return vec![None; count];
    };
    if count == 0 {
        return Vec::new();
    }
    let merged = union(intervals);
    let span = u128::from(window.end - window.start);
    let mut cursor = 0;
    (0..count)
        .map(|index| {
            // Products use u128; every resulting offset is bounded by the original u64 span.
            let offset = |position: usize| u64::try_from(span * position as u128 / count as u128).unwrap_or(u64::MAX);
            let start = window.start + offset(index);
            let end = window.start + offset(index + 1);
            while cursor < merged.len() && merged[cursor].end < start {
                cursor += 1;
            }
            let relevant = &merged[cursor..];
            let end_index = relevant.partition_point(|interval| interval.start < end || (index + 1 == count && interval.start == end));
            occupancy(&relevant[..end_index], TimeWindow { start, end })
        })
        .collect()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct HistogramBin {
    pub(super) lower: u64,
    pub(super) upper: u64,
    pub(super) count: u64,
}

// Inclusive nanosecond bounds: fixed decades from 0..<10ns through 10..<100s,
// followed by a 100s+ overflow bucket so even extreme durations are retained.
const HISTOGRAM_UPPER_BOUNDS: [u64; 12] = [
    9,
    99,
    999,
    9_999,
    99_999,
    999_999,
    9_999_999,
    99_999_999,
    999_999_999,
    9_999_999_999,
    99_999_999_999,
    u64::MAX,
];

pub(super) const HISTOGRAM_BINS: usize = HISTOGRAM_UPPER_BOUNDS.len();

/// Fixed log10 duration buckets, including zero in the first bucket. Adjacent
/// decades are grouped on very narrow panels; every sample is counted exactly once.
pub(super) fn histogram(samples: &[u64], count: usize) -> Vec<HistogramBin> {
    if count == 0 || samples.is_empty() {
        return Vec::new();
    }
    let count = count.min(HISTOGRAM_BINS);
    let mut result = (0..count)
        .map(|index| {
            let first = index * HISTOGRAM_BINS / count;
            let last = (index + 1) * HISTOGRAM_BINS / count;
            HistogramBin {
                lower: if first == 0 { 0 } else { HISTOGRAM_UPPER_BOUNDS[first - 1] + 1 },
                upper: HISTOGRAM_UPPER_BOUNDS[last - 1],
                count: 0,
            }
        })
        .collect::<Vec<_>>();
    for sample in samples {
        let index = result.partition_point(|bin| bin.upper < *sample);
        result[index].count += 1;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn median_is_exact_not_average_and_distinguishes_zero() {
        assert_eq!(
            (
                median_nanos(&mut []),
                median_nanos(&mut [0]),
                median_nanos(&mut [1, 2, 900]),
                median_nanos(&mut [9, 1, 4, 2]),
                median_nanos(&mut [u64::MAX, u64::MAX - 2]),
            ),
            (None, Some(0), Some(2), Some(3), Some(u64::MAX - 1))
        );
    }

    #[test]
    fn occupancy_uses_clipped_union_not_sum() {
        let intervals = [
            Interval { start: 0, end: 60 },
            Interval { start: 20, end: 90 },
            Interval { start: 80, end: 200 },
        ];
        assert_eq!(occupancy(&intervals, TimeWindow { start: 10, end: 100 }), Some(1.0));
        assert_eq!(occupancy(&[], TimeWindow { start: 10, end: 100 }), None);
        assert_eq!(occupancy(&intervals, TimeWindow { start: 10, end: 10 }), None);
        assert_eq!(
            occupancy(&[Interval { start: 15, end: 15 }], TimeWindow { start: 10, end: 100 }),
            Some(0.0)
        );
    }

    #[test]
    fn bins_preserve_unknown_gaps_and_common_axis() {
        assert_eq!(
            bins(&[Interval { start: 0, end: 50 }], Some(TimeWindow { start: 0, end: 100 }), 2),
            [Some(1.0), None]
        );
        assert_eq!(bins(&[], None, 3), [None, None, None]);
        assert_eq!(bins(&[], Some(TimeWindow { start: 0, end: u64::MAX }), 2), [None, None]);
    }

    #[test]
    fn histogram_retains_zero_and_extreme_samples_once() {
        let samples = [0, 0, 1, 2, 3, 4, 1_000_000, u64::MAX];
        let bins = histogram(&samples, 12);
        assert_eq!(bins.iter().map(|bin| bin.count).sum::<u64>(), 8);
        assert_eq!(
            bins.first(),
            Some(&HistogramBin {
                lower: 0,
                upper: 9,
                count: 6
            })
        );
        assert_eq!(bins.last().unwrap().upper, u64::MAX);
        assert!(bins.windows(2).all(|pair| pair[0].upper + 1 == pair[1].lower));
    }

    #[test]
    fn histogram_counts_both_sides_of_every_decade_boundary() {
        let samples = std::iter::once(0)
            .chain(
                HISTOGRAM_UPPER_BOUNDS[..HISTOGRAM_BINS - 1]
                    .iter()
                    .flat_map(|upper| [*upper, upper + 1]),
            )
            .chain([u64::MAX])
            .collect::<Vec<_>>();
        let bins = histogram(&samples, HISTOGRAM_BINS);
        assert_eq!(bins.iter().map(|bin| bin.count).collect::<Vec<_>>(), vec![2; 12]);
        assert_eq!(
            bins.iter().map(|bin| bin.lower).collect::<Vec<_>>(),
            [
                0,
                10,
                100,
                1_000,
                10_000,
                100_000,
                1_000_000,
                10_000_000,
                100_000_000,
                1_000_000_000,
                10_000_000_000,
                100_000_000_000
            ]
        );
    }

    #[test]
    fn histogram_keeps_fixed_buckets_for_small_samples_and_narrow_panels() {
        assert_eq!(histogram(&[0], HISTOGRAM_BINS).len(), 12);
        assert_eq!(histogram(&[1], usize::MAX).len(), 12);
        assert_eq!(
            histogram(&[0, 10, u64::MAX], 1),
            [HistogramBin {
                lower: 0,
                upper: u64::MAX,
                count: 3
            }]
        );
        for width in 1..HISTOGRAM_BINS {
            let bins = histogram(&[0, 9, 10, 99, 100, 1_000_000_000, u64::MAX], width);
            assert_eq!(bins.len(), width);
            assert_eq!(bins.iter().map(|bin| bin.count).sum::<u64>(), 7);
            assert!(bins.windows(2).all(|pair| pair[0].upper + 1 == pair[1].lower));
        }
        assert!(histogram(&[], HISTOGRAM_BINS).is_empty());
        assert!(histogram(&[1], 0).is_empty());
    }
}
