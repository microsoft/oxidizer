// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

export const columns = 12;

export function paneOrder(ids, order = []) {
  const known = new Set(ids);
  return [...new Set([...order.filter(id => known.has(id)), ...ids])];
}

export function movePane(order, id, target) {
  if (id === target || !order.includes(id) || !order.includes(target)) return order;
  const next = order.filter(item => item !== id);
  next.splice(order.indexOf(target), 0, id);
  return next;
}

export function paneSize(width, height) {
  return {
    width: Math.min(columns, Math.max(3, Math.round(width))),
    ...(Number.isFinite(height) && { height: Math.min(1200, Math.max(112, Math.round(height))) }),
  };
}

export function readLayouts(json) {
  if (!json) return {};
  const value = JSON.parse(json);
  if (!value || Array.isArray(value) || typeof value !== "object") return {};
  return Object.fromEntries(Object.entries(value).flatMap(([group, layout]) => {
    if (!layout || !Array.isArray(layout.order) || !layout.sizes || typeof layout.sizes !== "object") return [];
    const sizes = Object.fromEntries(Object.entries(layout.sizes).flatMap(([id, size]) =>
      Number.isFinite(size?.width) ? [[id, paneSize(size.width, size.height)]] : []));
    return [[group, { order: layout.order.filter(id => typeof id === "string"), sizes }]];
  }));
}
