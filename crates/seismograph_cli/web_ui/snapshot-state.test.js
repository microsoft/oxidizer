// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

import assert from "node:assert/strict";
import { test } from "node:test";
import { addSnapshot, closeSnapshot, pickDrillRow, readPage, resolveDrillRow, selectedRow } from "./snapshot-state.js";

test("refresh follows a thread even when rows with the same title change order", () => {
  const first = { title: "Worker", thread: { id: "1" }, hasChildren: true };
  const second = { title: "Worker", thread: { id: "2" }, hasChildren: true };
  const read = { title: "Read", hasChildren: true };
  const page = (rows, offset = 0) => ({ data: { rows }, offset });
  const oldThread = pickDrillRow(page([first, second]), second, "old");
  const oldOperation = pickDrillRow(page([read]), read, "old/1");
  const thread = resolveDrillRow(page([second, first]), oldThread, "new");
  assert.deepEqual(thread, { row: second, path: "0" });
  assert.equal(resolveDrillRow(page([{ title: "Write", hasChildren: true }]), oldOperation, "new/0", thread.path), null);
});

test("deep drill-down uses page offsets and keeps a leaf when its stack display changes", () => {
  const hotspot = { title: "app::allocate", stack: ["app::allocate"] };
  const displayed = { title: "allocator::allocate", stack: ["allocator::allocate", "app::allocate"] };
  const picked = pickDrillRow({ data: { rows: [hotspot] }, offset: 50 }, hotspot, "same-snapshot");
  assert.deepEqual(resolveDrillRow({ data: { rows: [displayed] }, offset: 50 }, picked, "same-snapshot", "2/9/1"), {
    row: displayed, path: "2/9/1/50",
  });
});

test("refresh keeps the current page until the new snapshot has fewer pages", async () => {
  const calls = [];
  const load = async offset => {
    calls.push(offset);
    return { total: 60, rows: [] };
  };
  assert.equal((await readPage(load, 50)).offset, 50);
  assert.equal((await readPage(load, 100)).offset, 50);
  assert.deepEqual(calls, [50, 100, 50]);
});

test("primitive drill-down keeps the selected operation after the grid changes order", () => {
  const deref = { title: "Deref", hasChildren: true };
  const clone = { title: "Clone", hasChildren: true };
  const picked = { key: "Deref", index: 0, scope: "events" };
  assert.equal(selectedRow({ offset: 0, data: { rows: [clone, deref] } }, picked, "objects"), deref);
  assert.equal(selectedRow({ offset: 0, data: { rows: [clone] } }, picked, "objects"), null);
});

test("recapture keeps the current view and makes the old capture independently selectable", () => {
  const first = addSnapshot({ tabs: [] }, { id: "1" }, "app");
  const next = addSnapshot(first, { id: "2" }, "app");
  assert.equal(next.tabs[1].viewKey, first.tabs[0].viewKey);
  assert.notEqual(next.tabs[0].viewKey, next.tabs[1].viewKey);
  assert.equal(closeSnapshot(next, "2").active, "1");
  const third = addSnapshot({ ...next, active: "1" }, { id: "3" }, "app");
  assert.equal(new Set(third.tabs.map(tab => tab.viewKey)).size, 3);
});
