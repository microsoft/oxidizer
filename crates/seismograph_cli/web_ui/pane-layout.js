// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

export function dockLayoutKey(section, ids) {
  return `seismograph.dockview.v1:${section}:${[...ids].sort().join(",")}`;
}

export function readDockLayout(json, ids) {
  if (json === null) return null;
  const layout = JSON.parse(json);
  const wanted = new Set(ids);
  const entries = Object.entries(layout?.panels ?? {});
  if (!layout?.grid?.root || entries.length !== wanted.size ||
      entries.some(([id, pane]) => !wanted.has(id) || pane?.contentComponent !== "report")) {
    throw new Error("The saved layout does not match these panes.");
  }
  if (layout.floatingGroups?.length || layout.popoutGroups?.length || layout.edgeGroups) {
    throw new Error("The saved layout contains panes outside the workspace.");
  }
  return layout;
}

export function dockPlacement(section, ids, index, width, height) {
  if (!index) return {};
  if (section === "runtime" && ids[index] === "task-detail") {
    return { position: { direction: "below" }, initialHeight: Math.max(180, Math.min(280, Math.round(height * .42))) };
  }
  const towardRight = width >= 540 && section !== "primitives" && index === 1;
  return {
    position: { referencePanel: ids[index - 1], direction: towardRight ? "right" : "below" },
    ...(towardRight
      ? { initialWidth: Math.round(width * (section === "runtime" ? .66 : .5)) }
      : { initialHeight: Math.max(180, Math.min(260, Math.round(height / 3))) }),
  };
}
