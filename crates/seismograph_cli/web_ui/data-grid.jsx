// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

import { useState } from "react";
import { gridRows } from "./grid-state.js";

export function DataGrid({ label, rows, columns, rowKey, selected, onSelect, paged, disabled, empty = "No items.", serverSort }) {
  const [query, setQuery] = useState("");
  const [localSort, setSort] = useState(null);
  const sort = columns.some(column => column.name === localSort?.column && !(column.sortKey && serverSort)) ? localSort : null;
  const visible = gridRows(rows, columns, query, sort);
  const [lastFocused, setLastFocused] = useState(null);
  const tabStop = [lastFocused, selected].find(key => visible.some(row => rowKey(row) === key))
    ?? (visible.length ? rowKey(visible[0]) : null);
  function moveFocus(event) {
    if (!event.target.matches(".row-link") || !["ArrowUp", "ArrowDown", "Home", "End"].includes(event.key)) return;
    const buttons = [...event.currentTarget.querySelectorAll(".row-link:not(:disabled)")];
    const index = buttons.indexOf(event.target);
    const destination = { ArrowUp: index - 1, ArrowDown: index + 1, Home: 0, End: buttons.length - 1 }[event.key];
    event.preventDefault();
    buttons[destination]?.focus();
  }
  return <div className="data-grid">
    <div className="grid-scroll">
      <table className="grid-table" aria-label={label}>
        <thead><tr><td className="grid-filter" colSpan={columns.length}>
          <input type="search" aria-label={`Filter ${label}`} placeholder={paged ? "Filter this page" : "Filter rows"}
            value={query} onChange={event => setQuery(event.target.value)} />
        </td></tr><tr>{columns.map(({ name, sortKey }) => {
          const remote = sortKey && serverSort;
          const current = sort ? sort.column === name : remote && serverSort.by === sortKey;
          const direction = current ? (sort ?? serverSort).descending ? "descending" : "ascending" : undefined;
          function sortColumn() {
            const descending = direction === "ascending";
            if (remote) {
              setSort(null);
              serverSort.onChange({ by: sortKey, descending });
            } else {
              setSort({ column: name, descending });
            }
          }
          return <th key={name} scope="col" aria-sort={direction}>
            <button type="button" title={paged ? remote ? "Sort all rows" : "Sort this page" : undefined} onClick={sortColumn}>
              {name}{direction && <span aria-hidden="true"> {direction === "ascending" ? "↑" : "↓"}</span>}
            </button>
          </th>;
        })}</tr></thead>
        <tbody onKeyDown={moveFocus}>{visible.map(row => {
          const key = rowKey(row);
          const pressed = selected === key;
          return <tr key={key} className={pressed ? "selected" : undefined}
            onClick={disabled ? undefined : event => onSelect(row, event.currentTarget.querySelector(".row-link"))}>
            {columns.map(({ name, value }, i) => i === 0
              ? <th scope="row" key={name}><button type="button" className="row-link" disabled={disabled}
                tabIndex={key === tabStop ? 0 : -1} onFocus={() => setLastFocused(key)}
                aria-current={pressed ? "true" : undefined}>{value(row)}</button></th>
              : <td key={name}>{value(row) ?? "Unknown"}</td>)}
          </tr>;
        })}
        {!visible.length && (rows.length > 0 || empty) && <tr className="grid-empty">
          <td colSpan={columns.length}>{rows.length ? "No rows match this filter." : empty}</td>
        </tr>}</tbody>
      </table>
    </div>
  </div>;
}

const detailOrigins = new WeakMap();

export function revealDetail(trigger, id) {
  const workspace = trigger.closest(".pane-workspace, .live");
  if (!workspace) return;
  const origins = detailOrigins.get(workspace) ?? new Map();
  origins.set(id, trigger);
  detailOrigins.set(workspace, origins);
  requestAnimationFrame(() => {
    const detail = workspace.querySelector(`[data-inspection="${id}"]`);
    const heading = detail?.querySelector(".section-heading");
    const main = workspace.closest("main");
    if (!heading || !main || workspace.closest("[hidden]")) return;
    const availableTop = Math.max(main.getBoundingClientRect().top,
      (workspace.closest(".snapshot")?.querySelector(".snapshot-toolbar")?.getBoundingClientRect().bottom ?? 0));
    const { top, bottom } = heading.getBoundingClientRect();
    const availableBottom = main.getBoundingClientRect().bottom;
    if (top < availableTop) main.scrollTop += top - availableTop - 8;
    else if (bottom + 56 > availableBottom) main.scrollTop += bottom + 56 - availableBottom;
  });
}

export function Detail({ id, title, close, heading: Heading = "h3", children }) {
  return <section className="detail-pane" data-inspection={id} aria-label={title}>
    <div className="section-heading"><Heading>{title}</Heading>
      {close && <button onClick={event => {
        const detail = event.currentTarget.closest(".detail-pane");
        const view = detail.closest(".snapshot, .live");
        const origins = detailOrigins.get(detail.closest(".pane-workspace, .live"));
        const origin = origins?.get(id);
        origins?.delete(id);
        close();
        requestAnimationFrame(() => {
          if (origin?.isConnected && !origin.closest("[hidden]")) origin.focus();
          else view?.querySelector(".view-title")?.focus({ preventScroll: true });
        });
      }} aria-label={`Close ${title}`}>Close</button>}
    </div>
    {children}
  </section>;
}
