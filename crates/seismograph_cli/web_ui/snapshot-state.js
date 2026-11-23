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
    return rows[picked.index - page.offset] ?? null;
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
  return { scope, key: selectionKey(row), index: page.offset + page.data.rows.indexOf(row), title: row.title, row };
}

export function resolveDrillRow(page, picked, scope, parent = "") {
  const onPage = selectedRow(page, picked, scope);
  const row = onPage ?? (picked?.scope === scope ? picked.row : null);
  if (!row) return null;
  const { index } = onPage ? pickDrillRow(page, row, scope) : picked;
  return { row, path: parent ? `${parent}/${index}` : String(index) };
}

const recentSnapshots = 4;

export function captureFor(workspace, source) {
  if (source === null) return null;
  return workspace.tabs.find(tab => tab.snapshot.id === workspace.active && tab.source === source)
    ?? workspace.tabs.findLast(tab => tab.source === source);
}

export function addSnapshot(workspace, snapshot, app, filter = null) {
  const source = app?.id ?? null;
  const captureCount = (workspace.captureCount ?? 0) + Number(app !== null);
  const label = app ? `Snapshot ${captureCount}` : snapshot.name.replace(/\.seismograph$/i, "") || snapshot.name;
  const application = app ? { id: app.id, name: app.name, instance: app.instance, pid: app.pid } : null;
  const prior = captureFor(workspace, source);
  const tabs = workspace.tabs.map(tab => tab === prior
    ? { ...tab, viewKey: `archive:${snapshot.id}` } : tab).slice(1 - recentSnapshots);
  tabs.push({ snapshot, source, application, label, filter, viewKey: prior?.viewKey ?? snapshot.id });
  return { tabs, active: snapshot.id, captureCount };
}

export function closeSnapshot(workspace, id) {
  const closed = workspace.tabs.find(tab => tab.snapshot.id === id);
  const tabs = workspace.tabs.filter(tab => tab.snapshot.id !== id);
  const next = tabs.findLast(tab => tab.source === closed?.source) ?? tabs.at(-1);
  return { ...workspace, tabs, active: workspace.active === id ? next?.snapshot.id : workspace.active };
}

export function snapshotGroups(apps, tabs) {
  const applications = new Map(apps.map(app => [app.id, { ...app, online: true, snapshots: [] }]));
  const files = [];
  for (const tab of tabs) {
    if (!tab.application) {
      files.push(tab);
      continue;
    }
    const { id } = tab.application;
    if (!applications.has(id)) applications.set(id, { ...tab.application, online: false, snapshots: [] });
    applications.get(id).snapshots.push(tab);
  }
  return { applications: [...applications.values()], files };
}
