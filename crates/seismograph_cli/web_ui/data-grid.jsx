// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

import { Fragment, useId, useLayoutEffect, useRef, useState } from "react";
import { gridRows } from "./grid-state.js";
import { revealDockedPane } from "./pane-workspace.jsx";
import { CloseButton } from "./view-chrome.jsx";

export function DataGrid({
  label, rows, columns, rowKey, selected, onSelect, paged, disabled, empty = "No items.", serverSort,
  renderChildren, canSelectRow, status, controls,
}) {
  const [query, setQuery] = useState("");
  const [filterOpen, setFilterOpen] = useState(false);
  const filterId = useId();
  const filterButton = useRef(null);
  const filterInput = useRef(null);
  const filterActive = !!query.trim();
  const [localSort, setSort] = useState(null);
  const sort = columns.some(column => column.name === localSort?.column && !(column.sortKey && serverSort)) ? localSort : null;
  const visible = gridRows(rows, columns, query, sort);
  const [lastFocused, setLastFocused] = useState(null);
  const selectable = !onSelect ? [] : canSelectRow ? visible.filter(canSelectRow) : visible;
  const tabStop = [lastFocused, selected].find(key => selectable.some(row => rowKey(row) === key))
    ?? (selectable.length ? rowKey(selectable[0]) : null);

  useLayoutEffect(() => {
    if (filterOpen) filterInput.current?.focus();
  }, [filterOpen]);

  function closeFilter() {
    setFilterOpen(false);
    filterButton.current?.focus();
  }

  function moveFocus(event) {
    if (!event.target.matches(".row-link") || !["ArrowUp", "ArrowDown", "Home", "End"].includes(event.key)) return;
    const buttons = [...event.currentTarget.querySelectorAll(".row-link:not(:disabled)")];
    const index = buttons.indexOf(event.target);
    const destination = { ArrowUp: index - 1, ArrowDown: index + 1, Home: 0, End: buttons.length - 1 }[event.key];
    event.preventDefault();
    const next = buttons[destination];
    if (next) {
      next.focus();
      const header = event.currentTarget.previousElementSibling.getBoundingClientRect();
      const scroller = next.closest(".dock-report") ?? next.closest(".grid-scroll");
      const top = next.getBoundingClientRect().top;
      if (header.bottom > top) scroller.scrollTop -= header.bottom - top;
    }
  }
  return <div className="data-grid" aria-busy={status?.loading || undefined}>
    <div className="grid-scroll" tabIndex={onSelect ? undefined : 0}
      role={onSelect ? undefined : "region"} aria-label={onSelect ? undefined : label}>
      <table className="grid-table" aria-label={label}>
        <thead><tr>{columns.map(({ name, sortKey, numeric }, index) => {
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
          const sortButton = <button type="button" title={paged ? remote ? "Sort all rows" : "Sort this page" : undefined} onClick={sortColumn}>
            {name}<span className={`sort-mark${direction ? " sorted" : ""}`} aria-hidden="true">
              {direction === "ascending" ? "↑" : direction === "descending" ? "↓" : "↕"}
            </span>
          </button>;
          return <th key={name} scope="col" className={numeric ? "numeric" : undefined} aria-sort={direction}>
            {index === 0 ? <div className="grid-title">
              {sortButton}
              {(rows.length > 0 || filterOpen || filterActive) && <button type="button" ref={filterButton}
                className={`grid-filter-toggle${filterActive ? " active" : ""}`}
                aria-label={`Filter ${label}${filterActive ? " (active)" : ""}`}
                title={filterActive ? "Filter is active" : paged ? "Filter this page" : "Filter rows"}
                aria-expanded={filterOpen} aria-controls={filterId}
                onClick={() => filterOpen ? closeFilter() : setFilterOpen(true)}>
                <svg viewBox="0 0 16 16" aria-hidden="true"><path d="M2.5 3.5h11L10 8v4l-4 1V8z" /></svg>
              </button>}
            </div> : sortButton}
          </th>;
        })}</tr>
        <tr hidden={!filterOpen && !controls}><td className="grid-filter" colSpan={columns.length}>
          <div className="grid-tools">
            <div id={filterId} className="grid-filter-input" hidden={!filterOpen}>
              <input ref={filterInput} type="search" aria-label={`Filter ${label}`}
                placeholder={paged ? "Filter this page" : "Filter rows"}
                value={query} onChange={event => setQuery(event.target.value)} onKeyDown={event => {
                  if (event.key === "Escape") {
                    event.preventDefault();
                    event.stopPropagation();
                    closeFilter();
                  }
                }} />
              {filterActive && <small role="status">{visible.length} of {rows.length} shown</small>}
            </div>
            {controls}
          </div>
        </td></tr></thead>
        <tbody onKeyDown={onSelect ? moveFocus : undefined}>{visible.map(row => {
          const key = rowKey(row);
          const pressed = onSelect && selected === key;
          const rowDisabled = disabled || (canSelectRow && !canSelectRow(row));
          const children = renderChildren?.(row);
          return <Fragment key={key}>
            <tr className={pressed ? "selected" : rowDisabled ? "disabled-row" : undefined}
              onClick={!onSelect || rowDisabled ? undefined : event => onSelect(row, event.currentTarget.querySelector(".row-link"))}>
              {columns.map(({ name, value, numeric }, i) => i === 0
                ? <th scope="row" key={name}>{onSelect ? <button type="button" className="row-link" disabled={rowDisabled}
                  title={String(value(row))}
                  tabIndex={key === tabStop ? 0 : -1} onFocus={() => setLastFocused(key)}
                  aria-current={pressed ? "true" : undefined}>{value(row)}</button> : value(row)}</th>
                : <td key={name} className={numeric ? "numeric" : undefined}>{value(row) ?? <span className="muted" title="Unknown">—</span>}</td>)}
            </tr>
            {children && <tr className="grid-children"><td colSpan={columns.length}>{children}</td></tr>}
          </Fragment>;
        })}
        {!visible.length && (status?.content || rows.length > 0 || empty) && <tr className="grid-empty">
          <td colSpan={columns.length}>{status?.content ?? (rows.length ? <>
            No rows match this filter. <button type="button" className="quiet" onClick={() => {
              setQuery("");
              (filterOpen ? filterInput : filterButton).current?.focus();
            }}>Clear filter</button>
          </> : empty)}</td>
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
    const heading = detail?.querySelector(":scope > .section-heading, :scope > h3");
    const main = workspace.closest("main");
    if (!heading || !main || workspace.closest("[hidden]")) return;
    revealDockedPane(detail);
    heading.scrollIntoView({ block: "nearest", inline: "nearest" });
  });
}

export function Detail({ id, title, close, heading: Heading = "h3", children }) {
  return <section className="detail-pane" data-inspection={id} aria-label={title}>
    <div className="section-heading"><Heading>{title}</Heading>
      {close && <CloseButton label={`Close ${title}`} onClick={event => {
        const detail = event.currentTarget.closest(".detail-pane");
        const view = detail.closest(".snapshot, .live");
        const origins = detailOrigins.get(detail.closest(".pane-workspace, .live"));
        const origin = origins?.get(id);
        origins?.delete(id);
        close();
        requestAnimationFrame(() => {
          if (origin?.isConnected && !origin.closest("[hidden]")) {
            revealDockedPane(origin);
            origin.focus();
          }
          if (!origin || document.activeElement !== origin) view?.querySelector(".view-title")?.focus({ preventScroll: true });
        });
      }} />}
    </div>
    {children}
  </section>;
}
