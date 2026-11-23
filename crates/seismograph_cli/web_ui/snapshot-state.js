// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

export const pageSize = 50;
export const offsetOf = index => Math.floor(index / pageSize) * pageSize;

export function selectionKey(row) {
  if (row.thread) return `thread:${row.thread.id ?? "0"}`;
  if (row.ioResource) return `io:${row.ioResource.id ?? "0"}`;
  if (row.cacheTier) return `cache:${row.cacheTier.id ?? "0"}:${!!row.cacheTier.fallback}`;
  const relatedThread = row.fields?.find(field => field.name === "Thread");
  return relatedThread ? `peer:${relatedThread.value}` : row.title;
}

export function selectedRow(page, picked, scope) {
  if (!picked || !page.data?.rows) return null;
  const rows = page.data.rows;
  if (picked.scope === scope) {
    const row = rows[picked.index - page.offset];
    if (row) return row;
  }
  const matches = rows.filter(row => selectionKey(row) === picked.key);
  return matches.length === 1 ? matches[0] : null;
}

export async function readPage(load, offset) {
  const data = await load(offset);
  const last = offsetOf(Math.max(0, (data.total ?? 0) - 1));
  if (offset > last) return { offset: last, data: await load(last) };
  return { offset, data };
}

export function pickDrillRow(page, row, scope) {
  return { scope, key: selectionKey(row), index: page.offset + page.data.rows.indexOf(row), title: row.title };
}

export function resolveDrillRow(page, picked, scope, parent = "") {
  const row = selectedRow(page, picked, scope);
  if (!row) return null;
  const { index } = pickDrillRow(page, row, scope);
  return { row, path: parent ? `${parent}/${index}` : String(index) };
}

const recentSnapshots = 4;

export function addSnapshot(workspace, snapshot, source, filter = null) {
  const active = workspace.tabs.find(tab => tab.snapshot.id === workspace.active);
  const preserve = source !== null && active?.source === source;
  const tabs = workspace.tabs.map(tab => tab === active && preserve
    ? { ...tab, viewKey: `archive:${snapshot.id}` } : tab).slice(1 - recentSnapshots);
  tabs.push({ snapshot, source, filter, viewKey: preserve ? active.viewKey : snapshot.id });
  return { tabs, active: snapshot.id };
}

export function closeSnapshot(workspace, id) {
  const tabs = workspace.tabs.filter(tab => tab.snapshot.id !== id);
  return { tabs, active: workspace.active === id ? tabs.at(-1)?.snapshot.id : workspace.active };
}
