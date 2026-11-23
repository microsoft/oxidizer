// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

import assert from "node:assert/strict";
import { test } from "node:test";
import { dockLayoutKey, dockPlacement, readDockLayout } from "./pane-layout.js";

test("a closed detail does not replace the layout saved for when it was open", () => {
  const layout = {
    grid: { root: { type: "branch", data: [] } },
    panels: {
      events: { contentComponent: "report" },
      operations: { contentComponent: "report" },
    },
  };
  const open = dockLayoutKey("primitives", ["events", "operations"]);
  assert.equal(open, dockLayoutKey("primitives", ["operations", "events"]));
  assert.notEqual(open, dockLayoutKey("primitives", ["events"]));
  assert.deepEqual(readDockLayout(JSON.stringify(layout), ["events", "operations"]), layout);
  assert.throws(() => readDockLayout(JSON.stringify(layout), ["events"]), /does not match/);
});

test("opening primitive details keeps parent panes above them, even on a wide screen", () => {
  const panes = ["events", "operations", "hotspots"];
  assert.deepEqual(dockPlacement("primitives", panes, 1, 1400, 780).position, { referencePanel: "events", direction: "below" });
  assert.deepEqual(dockPlacement("primitives", panes, 2, 1400, 780).position, { referencePanel: "operations", direction: "below" });
});

test("Runtime opens Task Details below the whole workspace, not beside either parent", () => {
  const panes = ["workers", "tasks", "task-detail"];
  for (const width of [390, 800, 1400]) {
    assert.deepEqual(dockPlacement("runtime", panes, 2, width, 780).position, { direction: "below" });
  }
});
