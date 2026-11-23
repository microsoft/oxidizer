// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

import assert from "node:assert/strict";
import { test } from "node:test";
import { sample } from "./activity.js";

test("activity uses exact counter deltas and elapsed time", () => {
  const first = sample({ sessionId: "7", classEvents: ["9007199254740993000", "7"] }, {}, 100);
  const next = sample({ sessionId: "7", classEvents: ["9007199254740993003", "7"] }, first, 1600);
  assert.deepEqual(next.history, [{ time: 1600, rates: [2, 0] }]);
});

test("activity establishes a new baseline after reset or a failed read", () => {
  const first = sample({ sessionId: "7", classEvents: ["100"] }, {}, 100);
  const next = sample({ sessionId: "7", classEvents: ["108"] }, first, 1100);
  assert.deepEqual(sample({ sessionId: "8", classEvents: ["4"] }, next, 2100).history, []);
  assert.deepEqual(sample({ sessionId: "7", classEvents: ["121"] }, { ...next, error: "Unavailable" }, 3100).history, []);
});

test("sparse event classes keep a nonzero rate", () => {
  const first = sample({ sessionId: "7", classEvents: ["10"] }, {}, 100);
  const second = sample({ sessionId: "7", classEvents: ["11"] }, first, 4100);
  assert.deepEqual(second.history, [{ time: 4100, rates: [0.25] }]);
});
