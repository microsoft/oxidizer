// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

import assert from "node:assert/strict";
import { test } from "node:test";
import { movePane, paneOrder, paneSize, readLayouts } from "./pane-layout.js";

test("moving an open pane does not lose the place of a closed detail", () => {
  const order = movePane(paneOrder(["events", "operations", "deref"]), "operations", "events");
  assert.deepEqual(paneOrder(["events", "operations"], order), ["operations", "events"]);
  assert.deepEqual(paneOrder(["events", "operations", "deref"], order), ["operations", "events", "deref"]);
});

test("saved widths and heights are bounded before reopening a view", () => {
  const runtime = { order: ["tasks", "workers", 17], sizes: { tasks: { width: 20, height: 75 }, invalid: { width: "wide" } } };
  const loaded = readLayouts(JSON.stringify({ runtime }));
  assert.deepEqual(loaded.runtime, { order: ["tasks", "workers"], sizes: { tasks: { width: 12, height: 112 } } });
  assert.deepEqual(paneSize(6, undefined), { width: 6 });
});
