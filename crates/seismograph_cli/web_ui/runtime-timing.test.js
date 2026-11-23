// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

import assert from "node:assert/strict";
import { test } from "node:test";
import { timingSeries } from "./runtime-timing.js";

test("timing omits missing tracks but keeps an observed zero and its position", () => {
  const measured = { title: "Polls", bins: [{}, { fraction: 0 }, {}] };
  const { timelines } = timingSeries({ timelines: [{ title: "Ready", bins: [{}, {}] }, measured] });
  assert.deepEqual(timelines, [measured]);
});

test("timing drops empty distributions and exposes the exact count and range for a single bucket", () => {
  const buckets = [{}, { lowerNanos: "10", upperNanos: "99", count: "9007199254740993" }, { count: "0" }];
  const { histograms } = timingSeries({ histograms: [{ title: "Empty", buckets: [{}] }, { title: "Polls", buckets }] });
  assert.deepEqual(histograms, [{
    title: "Polls",
    buckets: [{ lowerNanos: "10", upperNanos: "99", count: 9007199254740993n }],
  }]);
});

test("timing keeps gaps between populated duration buckets", () => {
  const buckets = [{}, { count: "1" }, {}, { count: "3" }, {}];
  const { histograms } = timingSeries({ histograms: [{ title: "Polls", buckets }] });
  assert.deepEqual(histograms[0].buckets.map(bucket => bucket.count), [1n, 0n, 3n]);
});
