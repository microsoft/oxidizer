// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

export function timingSeries({ timelines = [], histograms = [] }) {
  return {
    // A measured zero is an observation. Only omit tracks with no measurements.
    timelines: timelines.filter(track => track.bins?.some(bin => bin.fraction != null)),
    histograms: histograms.flatMap(({ title, buckets = [] }) => {
      const counts = buckets.map(bucket => BigInt(bucket.count ?? 0));
      const first = counts.findIndex(count => count > 0n);
      if (first < 0) return [];
      const last = counts.findLastIndex(count => count > 0n);
      return [{
        title,
        // Keep empty buckets between observations so the chart still shows gaps.
        buckets: buckets.slice(first, last + 1).map((bucket, index) => ({
          ...bucket, count: counts[first + index],
        })),
      }];
    }),
  };
}
