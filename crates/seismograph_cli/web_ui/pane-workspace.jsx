// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

import { createContext, useContext, useEffect, useId, useRef, useState } from "react";
import { columns, movePane, paneOrder, paneSize, readLayouts } from "./pane-layout.js";

const storageKey = "seismograph.panes.v1";
const Layouts = createContext(null);
const emptyLayout = { order: [], sizes: {} };

export function PaneLayoutProvider({ children }) {
  const [stored, setStored] = useState(() => {
    try {
      return { layouts: readLayouts(localStorage.getItem(storageKey)), warning: "" };
    } catch (error) {
      return { layouts: {}, warning: `Saved layouts could not be loaded. ${error.message}` };
    }
  });
  const { layouts, warning } = stored;
  const [changed, setChanged] = useState(false);
  useEffect(() => {
    if (!changed) return;
    try {
      localStorage.setItem(storageKey, JSON.stringify(layouts));
      setStored(value => value.warning ? { ...value, warning: "" } : value);
    } catch (error) {
      setStored(value => ({ ...value, warning: `Layouts could not be saved in this browser. ${error.message}` }));
    }
  }, [layouts, changed]);

  function update(group, change) {
    setStored(value => {
      const current = value.layouts[group] ?? emptyLayout;
      const next = change(current);
      return { ...value, layouts: { ...value.layouts, [group]: next } };
    });
    setChanged(true);
  }
  return <Layouts.Provider value={{ layouts, warning, update }}>{children}</Layouts.Provider>;
}

export function PaneWorkspace({ group, panes, editing, hidden }) {
  const instructions = useId();
  const { layouts, warning, update } = useContext(Layouts);
  const layout = layouts[group] ?? emptyLayout;
  const ids = panes.map(pane => pane.id);
  const order = paneOrder(ids, layout.order);
  const visible = order.filter(id => panes.find(pane => pane.id === id).content);
  const grid = useRef(null);
  const resize = useRef(null);
  const drag = useRef(null);
  const [preview, setPreview] = useState(null);
  const [dragging, setDragging] = useState(null);
  const [target, setTarget] = useState(null);

  function saveSize(id, size) {
    update(group, current => ({ ...current, sizes: { ...current.sizes, [id]: size } }));
  }

  function move(id, destination) {
    if (!destination || destination === id) return;
    update(group, current => ({ ...current, order: movePane(paneOrder(ids, current.order), id, destination) }));
  }

  function startResize(event, id, size) {
    if (event.button !== 0 || !event.isPrimary) return;
    event.preventDefault();
    const handle = event.currentTarget;
    handle.focus();
    const pane = handle.closest(".workspace-pane");
    const cells = getComputedStyle(grid.current).gridTemplateColumns.split(" ");
    const unit = cells.length === columns ? parseFloat(cells[0]) + parseFloat(getComputedStyle(grid.current).columnGap) : 0;
    resize.current = { id, size, unit, height: pane.getBoundingClientRect().height, x: event.clientX, y: event.clientY };
    handle.setPointerCapture(event.pointerId);
  }

  function calculateSize(event) {
    const start = resize.current;
    const width = start.unit > 0 ? start.size.width + (event.clientX - start.x) / start.unit : start.size.width;
    return paneSize(width, start.height + event.clientY - start.y);
  }

  function finishResize(event, commit) {
    if (!resize.current) return;
    if (commit && (event.clientX !== resize.current.x || event.clientY !== resize.current.y)) {
      saveSize(resize.current.id, calculateSize(event));
    }
    resize.current = null;
    setPreview(null);
  }

  function moveTarget(event) {
    const element = document.elementFromPoint(event.clientX, event.clientY)?.closest("[data-pane-id]");
    return element?.parentElement === grid.current ? element.dataset.paneId : null;
  }

  function finishMove(event, commit) {
    if (drag.current && commit) move(drag.current, moveTarget(event));
    drag.current = null;
    setDragging(null);
    setTarget(null);
  }

  return <div className="pane-workspace" hidden={hidden}>
    {editing && <div className="actions pane-layout-tools">
      <span className="muted" id={instructions}>Drag a handle or use its arrow keys. Press Home on Resize to restore its size.</span>
      <button onClick={() => update(group, () => emptyLayout)}>Reset layout</button>
    </div>}
    {warning && <p className="error" role="status">{warning}</p>}
    <div className="pane-grid" ref={grid}>
      {visible.map((id, index) => {
        const { title, content, width = columns, wideWidth } = panes.find(pane => pane.id === id);
        const custom = preview?.id === id ? preview.size : layout.sizes[id];
        const style = {
          "--pane-width": custom?.width,
          "--default-pane-width": width,
          "--wide-pane-width": wideWidth,
          "--pane-height": custom?.height ? `${custom.height}px` : undefined,
        };
        function currentWidth(element) {
          const span = getComputedStyle(element.closest(".workspace-pane")).gridColumnEnd.match(/^span (\d+)$/);
          return custom?.width ?? (span ? Number(span[1]) : width);
        }
        function resizeWithKey(event) {
          if (!["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "Home"].includes(event.key)) return;
          event.preventDefault();
          const height = custom?.height ?? event.currentTarget.closest(".workspace-pane").getBoundingClientRect().height;
          const delta = event.key === "ArrowRight" ? 1 : event.key === "ArrowLeft" ? -1 : 0;
          const heightChange = event.key === "ArrowDown" ? 24 : event.key === "ArrowUp" ? -24 : 0;
          if (event.key === "Home") {
            update(group, current => {
              const { [id]: _, ...sizes } = current.sizes;
              return { ...current, sizes };
            });
          } else {
            saveSize(id, paneSize(currentWidth(event.currentTarget) + delta, heightChange ? height + heightChange : custom?.height));
          }
        }
        return <div key={id} data-pane-id={id} className="workspace-pane" style={style} data-sized={custom?.height ? "" : undefined}
          data-drop-target={target === id && dragging !== id ? "" : undefined}>
          {editing && <div className="actions pane-handles">
            <button type="button" className="pane-move" aria-label={`Move ${title}`} aria-describedby={instructions}
              onPointerDown={event => {
                if (event.button !== 0 || !event.isPrimary) return;
                event.preventDefault();
                event.currentTarget.focus();
                drag.current = id;
                setDragging(id);
                event.currentTarget.setPointerCapture(event.pointerId);
              }}
              onPointerMove={event => drag.current === id && setTarget(moveTarget(event))}
              onPointerUp={event => finishMove(event, true)}
              onPointerCancel={event => finishMove(event, false)}
              onLostPointerCapture={event => finishMove(event, false)}
              onKeyDown={event => {
                const delta = ["ArrowUp", "ArrowLeft"].includes(event.key) ? -1 : ["ArrowDown", "ArrowRight"].includes(event.key) ? 1 : 0;
                if (!delta) return;
                event.preventDefault();
                move(id, visible[index + delta]);
              }}>Move</button>
            <button type="button" className="pane-resize" aria-label={`Resize ${title}`} aria-describedby={instructions}
              onPointerDown={event => startResize(event, id, { width: currentWidth(event.currentTarget) })}
              onPointerMove={event => resize.current?.id === id && setPreview({ id, size: calculateSize(event) })}
              onPointerUp={event => finishResize(event, true)}
              onPointerCancel={event => finishResize(event, false)}
              onLostPointerCapture={event => finishResize(event, false)}
              onKeyDown={resizeWithKey}>Resize</button>
            <small>{title}</small>
          </div>}
          <div className="workspace-pane-body">{content}</div>
        </div>;
      })}
    </div>
  </div>;
}
