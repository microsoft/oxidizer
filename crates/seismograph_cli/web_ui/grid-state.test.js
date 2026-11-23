// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

import assert from "node:assert/strict";
import { test } from "node:test";
import { gridRows } from "./grid-state.js";

test("grid filters across columns and sorts exact counters without moving unknowns first", () => {
  const rows = [
    { kind: "Mutex A", events: "9007199254740992" },
    { kind: "Mutex B", events: "9007199254740993" },
    { kind: "Mutex C" },
    { kind: "Arc", events: "7" },
  ];
  const columns = [
    { name: "Primitive", value: row => row.kind },
    { name: "Events", value: row => row.events },
  ];
  assert.deepEqual(gridRows(rows, columns, "mutex", { column: "Events", descending: true }), [rows[1], rows[0], rows[2]]);
  assert.deepEqual(gridRows(rows, columns, "9007199254740993"), [rows[1]]);
});
