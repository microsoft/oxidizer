// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

import { createContext, useContext, useEffect, useLayoutEffect, useRef, useState } from "react";
import { DockviewDefaultTab, DockviewReact } from "dockview-react";
import "dockview-react/dist/styles/dockview.css";
import { dockLayoutKey, dockPlacement, readDockLayout } from "./pane-layout.js";

const PaneContents = createContext([]);
const docks = new WeakMap();

function ReportPane({ api }) {
  const pane = useContext(PaneContents).find(item => item.id === api.id);
  return <div className="dock-report" data-dock-pane={api.id}>{pane?.content}</div>;
}

function ReportTab(props) {
  return <DockviewDefaultTab {...props} hideClose />;
}

const components = { report: ReportPane };
const noTabActions = () => [];

export function revealDockedPane(detail) {
  const id = detail.closest("[data-dock-pane]")?.dataset.dockPane;
  const pane = id && docks.get(detail.closest(".pane-workspace"))?.getPanel(id);
  if (pane && !pane.api.isVisible) pane.api.setActive();
}

export function PaneWorkspace({ group, panes, hidden }) {
  const workspace = useRef(null);
  const layout = useRef(null);
  const [api, setApi] = useState(null);
  const [warning, setWarning] = useState("");
  const available = panes.filter(pane => pane.content);
  const ids = available.map(pane => pane.id);
  const key = dockLayoutKey(group, ids);

  function addMissingPanels(dock) {
    available.forEach(({ id, title }, index) => {
      if (dock.getPanel(id)) return;
      dock.addPanel({
        id, title, component: "report", renderer: "always", minimumHeight: 112, minimumWidth: 132,
        ...dockPlacement(group, ids, index, dock.width, dock.height),
      });
    });
  }

  useLayoutEffect(() => {
    if (!api || hidden || layout.current === key) return;
    const existing = new Set(api.panels.map(panel => panel.id));
    // Opening a pane can restore its last position. Closing one leaves the remaining panes in place.
    if (ids.some(id => !existing.has(id))) {
      try {
        const saved = readDockLayout(localStorage.getItem(key), ids);
        if (saved) api.fromJSON(saved, { reuseExistingPanels: true });
      } catch (error) {
        setWarning(`Could not restore the saved layout. ${error.message}`);
      }
    }
    for (const panel of api.panels) {
      if (!ids.includes(panel.id)) api.removePanel(panel);
    }
    addMissingPanels(api);
    layout.current = key;
  }, [api, hidden, key]);

  useEffect(() => {
    if (!api || hidden) return;
    const subscription = api.onDidLayoutChange(({ reasons }) => {
      if (layout.current !== key || api.width === 0 ||
          (!reasons.has("geometry") && !reasons.has("structure"))) return;
      try {
        localStorage.setItem(key, JSON.stringify(api.toJSON()));
      } catch (error) {
        setWarning(`Could not save the layout. ${error.message}`);
      }
    });
    return () => subscription.dispose();
  }, [api, hidden, key]);

  function resetLayout() {
    if (!api) return;
    try {
      const prefix = `seismograph.dockview.v1:${group}:`;
      for (let index = localStorage.length - 1; index >= 0; index--) {
        const saved = localStorage.key(index);
        if (saved?.startsWith(prefix)) localStorage.removeItem(saved);
      }
      setWarning("");
    } catch (error) {
      setWarning(`Could not clear the saved layout. ${error.message}`);
    }
    api.clear();
    addMissingPanels(api);
  }

  const minHeight = `${(group === "primitives" ? Math.max(ids.length, 2) : Math.max(ids.length - 1, 2)) * 9}rem`;
  return <div className="pane-workspace" ref={workspace} hidden={hidden}>
    {group !== "overview" && <div className="actions pane-layout-tools">
      <button type="button" className="quiet" title="Drag a pane tab to dock it. Drag a divider to resize." onClick={resetLayout}>Reset layout</button>
    </div>}
    {warning && <p className="error" role="status">{warning}</p>}
    {(!hidden || api) && <PaneContents.Provider value={panes}>
      <div className="dock-container" style={{ "--dock-min-height": minHeight }} onKeyDownCapture={event => {
        // Report selection owns pane removal, not Dockview's tab deletion shortcut.
        if ((event.key === "Delete" || event.key === "Backspace") && event.target.closest(".dv-tab")) {
          event.preventDefault();
          event.stopPropagation();
        }
      }}>
        <DockviewReact className="dockview-theme-light monitor-dock" components={components}
          defaultTabComponent={ReportTab} getTabContextMenuItems={noTabActions}
          disableFloatingGroups keyboardNavigation
          onReady={({ api: dock }) => {
            docks.set(workspace.current, dock);
            setApi(dock);
          }} />
      </div>
    </PaneContents.Provider>}
  </div>;
}
