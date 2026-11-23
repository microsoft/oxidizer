// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

import { useState } from "react";
import { api, download } from "./api.js";
import { DataGrid, Detail, revealDetail } from "./data-grid.jsx";
import { PaneWorkspace } from "./pane-workspace.jsx";
import { useSnapshotPage } from "./snapshot-page.js";
import { pageSize, pickDrillRow, resolveDrillRow } from "./snapshot-state.js";

const initialFilter = { include: "", exclude: "", includeUnknown: false, fullStack: false };
const reportSorts = {
  primitives: { Events: "events", Objects: "objects", Threads: "threads", Hotspots: "hotspots" },
  allocations: {
    "Allocated bytes": "allocated", Allocations: "count", "Average allocation bytes": "average",
    "Live bytes": "live", "Live allocations": "live-count",
  },
};
const help = {
  overview: "A snapshot contains retained events and any allocator and runtime state recorded at capture.",
  allocations: "Live bytes describe retained evidence, not a census of all allocations. Open a row to inspect its stack.",
  snmalloc: "Reservations are virtual address space, not physical memory. Each owner and range walk has its own coverage.",
  mimalloc: "Arena commitment is not physical residency. Older observations describe past state. Missing measurements are unknown.",
  runtime: "Execution metrics use retained polls. Task operation counts are inferred from recorder activity.",
  threads: "Recorder thread IDs identify recorded thread lifetimes, not operating system thread IDs.",
  primitives: "Select a primitive type to see its operations below. Select an operation to see its stack hotspots.",
  io: "Operations can be incomplete when the snapshot does not contain both start and finish events.",
  cache: "These counts reflect retained events, not lifetime cache totals.",
};
const labels = {
  id: "ID", runtimeId: "Runtime ID", workerId: "Worker ID", taskId: "Task ID", threadId: "Thread ID",
  parentId: "Parent task", io: "I/O", pollWorkerId: "Poll worker", observedTasks: "Observed tasks",
  maxPollNanos: "Maximum poll (ns)", medianPollNanos: "Median poll (ns)",
  runningNanos: "Running for (ns)", readyNanos: "Ready for (ns)", executingFraction: "Observed execution",
};
const label = key => labels[key] ?? key.replace(/[A-Z]/g, match => ` ${match.toLowerCase()}`).replace(/^./, c => c.toUpperCase());
const format = new Intl.NumberFormat();
function formatValue(key, value) {
  if (value == null) return "Unknown";
  if (key === "executingFraction") return `${(value * 100).toFixed(1)}%`;
  if (typeof value === "boolean") return value ? "Yes" : "No";
  if (Array.isArray(value)) return value.length ? value.join(", ") : "None";
  if (typeof value === "number" || (typeof value === "string" && /^\d+$/.test(value) && !key.endsWith("Id") && key !== "id")) {
    return format.format(typeof value === "string" ? BigInt(value) : value);
  }
  return String(value);
}

const models = {
  runtimeWorker: { required: { runtimeId: "0", observedTasks: "0" }, optional: ["workerId", "threadId", "currentTask"] },
  runtimeTask: {
    required: { runtimeId: "0", taskId: "0", workerIds: [], repollRequested: false },
    optional: ["parentId", "futureSizeBytes", "runningNanos", "readyNanos", "pollWorkerId", "typeDescriptorId"],
  },
  thread: { required: { id: "0", total: "0", retained: "0", lost: "0" }, optional: [] },
  allocationHotspot: { required: { allocations: "0", allocatedBytes: "0", liveAllocations: "0", liveBytes: "0" }, optional: [] },
  ioResource: { required: { id: "0", reads: "0", writes: "0", completed: "0", errors: "0", canceled: "0", requestedBytes: "0", completedBytes: "0" }, optional: [] },
  cacheTier: { required: { id: "0", fallback: false, events: "0", hits: "0", misses: "0", errors: "0" }, optional: [] },
  primitiveGroup: { required: { events: "0", objects: "0", contentions: "0" }, optional: [] },
  taskOccurrence: { required: { objectId: "0", threadId: "0", sequence: "0", timestampNanos: "0", relativeStackKnown: false }, optional: [] },
};

function rowFields(row) {
  for (const [type, definition] of Object.entries(models)) {
    const model = row[type];
    if (!model) continue;
    const { execution, ...properties } = model;
    const values = { ...definition.required, ...Object.fromEntries(definition.optional.map(key => [key, null])), ...properties };
    if (execution) {
      Object.assign(values, { polls: "0", executingFraction: null, medianPollNanos: null, maxPollNanos: null }, execution);
    }
    return [
      ...Object.entries(values).map(([key, value]) => ({ name: label(key), value: formatValue(key, value), raw: value })),
      ...(row.fields ?? []),
    ];
  }
  return row.fields ?? [];
}

export function Snapshot({ snapshot, active, appliedFilter, onFilter, onClose, action, busy }) {
  const [section, setSection] = useState("overview");
  const [fullStack, setFullStack] = useState(false);
  const [editing, setEditing] = useState(false);
  const [showFilter, setShowFilter] = useState(false);
  const [filter, setFilter] = useState(appliedFilter ?? initialFilter);
  const activeSection = snapshot.sections.find(item => item.id === section) ?? snapshot.sections[0];

  function applyFilter(nextFilter) {
    void action("Applying stack filter", async () => {
      const updated = await api.filter(snapshot.id, nextFilter);
      setFilter(nextFilter);
      onFilter(updated, nextFilter);
    });
  }

  return <section id={`snapshot-${snapshot.id}`} className="snapshot" aria-label="Snapshot" hidden={!active}>
    <div className="page-heading snapshot-heading"><h2 className="view-title" tabIndex={-1} title={snapshot.name}>{snapshot.name}</h2>
      <div className="actions">
        <button aria-pressed={editing} onClick={() => setEditing(value => !value)}>{editing ? "Done arranging" : "Arrange views"}</button>
        <button disabled={!!busy} title={snapshot.savedPath ? `Saved to ${snapshot.savedPath}` : undefined}
          onClick={() => action("Downloading snapshot", () => download(snapshot.id, snapshot.name))}>Download</button>
        <button aria-expanded={showFilter} onClick={() => setShowFilter(value => !value)}>Stack filter</button>
        <button disabled={!!busy} onClick={onClose}>Close snapshot</button>
      </div>
    </div>
    <div className="snapshot-toolbar">
      <nav className="sections" aria-label="Snapshot sections">{snapshot.sections.map(item =>
        <button key={item.id} aria-current={activeSection.id === item.id ? "page" : undefined} onClick={() => setSection(item.id)}>{item.title}</button>
      )}</nav>
      <div className="actions">
        <label><input type="checkbox" checked={fullStack} onChange={event => setFullStack(event.target.checked)} /> Full stacks</label>
        <details className="snapshot-help"><summary aria-label={`About ${activeSection.title}`}>Help</summary>
          <p>{help[activeSection.id]}{snapshot.savedPath && <span className="saved-path">Saved to <code>{snapshot.savedPath}</code></span>}</p>
        </details>
      </div>
    </div>
    {snapshot.warnings?.map(warning => <p className="notice" key={warning}>{warning}</p>)}
    {!!snapshot.filterSummary?.length && <Fields fields={snapshot.filterSummary} />}
    {showFilter && <form className="surface" onSubmit={event => { event.preventDefault(); applyFilter(filter); }}>
      <h3>Stack filter</h3>
      <p className="muted">Separate rules with spaces or commas. Use <code>crate:name</code>, <code>module:name::path</code>, or <code>function:name::path</code>.</p>
      <div className="filter-fields">
        <label>Include<textarea value={filter.include} onChange={event => setFilter({ ...filter, include: event.target.value })} /></label>
        <label>Exclude<textarea value={filter.exclude} onChange={event => setFilter({ ...filter, exclude: event.target.value })} /></label>
      </div>
      <div className="actions">
        <label><input type="checkbox" checked={filter.includeUnknown} onChange={event => setFilter({ ...filter, includeUnknown: event.target.checked })} /> Include unknown stacks</label>
        <label><input type="checkbox" checked={filter.fullStack} onChange={event => setFilter({ ...filter, fullStack: event.target.checked })} /> Match runtime spawn stacks</label>
        <button className="primary" disabled={!!busy}>Apply filter</button>
        <button type="button" disabled={!!busy} onClick={() => applyFilter(initialFilter)}>Reset filter</button>
      </div>
    </form>}
    <SnapshotOverview snapshot={snapshot} active={active && activeSection.id === "overview"} editing={editing} />
    <RuntimePanel snapshot={snapshot} active={active && activeSection.id === "runtime"} fullStack={fullStack} editing={editing} />
    {snapshot.sections.filter(item => !["overview", "runtime"].includes(item.id)).map(item =>
      <SnapshotSection key={item.id} snapshot={snapshot} section={item} active={active && activeSection.id === item.id}
        fullStack={fullStack} editing={editing} />
    )}
  </section>;
}

function SnapshotOverview({ snapshot, active, editing }) {
  const page = useSnapshotPage({ snapshot, section: "overview", offset: 0, fullStack: false, by: "", descending: false, active });
  return <PaneWorkspace group="overview" editing={editing} hidden={!active} panes={[
    { id: "summary", title: "Snapshot summary", content: <>
      <PageStatus page={page} label="Overview" />
      <div className="overview-stats">{page.data?.rows?.map(row => <section key={row.title} className="surface" aria-label={row.title}>
        <h3>{row.title}</h3><Fields fields={rowFields(row)} />
      </section>)}</div>
    </> },
  ]} />;
}

function useReportLevel(options, offset, picked, sort, previous) {
  const parent = previous?.selection?.path ?? "";
  const active = options.active && (!previous || !!previous.selection?.row.hasChildren);
  const scope = JSON.stringify([options.snapshot.id, options.snapshot.viewId, parent, sort]);
  const page = useSnapshotPage({ ...options, ...sort, parent, offset, active });
  const selection = active ? resolveDrillRow(page, picked, scope, parent) : null;
  return { page, selection, parent, scope };
}

const listLabels = {
  primitives: ["Primitive events", "operations", "hotspots"],
  threads: ["Threads", "operations", "related threads", "objects"],
  io: ["I/O resources", "operations"],
  cache: ["Cache tiers", "operations"],
};

function SnapshotSection({ snapshot, section, active, fullStack, editing }) {
  const [picked, setPicked] = useState([]);
  const [offsets, setOffsets] = useState([]);
  const [sorting, setSorting] = useState({ by: Object.values(reportSorts[section.id] ?? {})[0] ?? "", descending: true });
  const options = { snapshot, section: section.id, fullStack, active };
  const root = useReportLevel(options, offsets[0] ?? 0, picked[0], { by: "", descending: true });
  const first = useReportLevel(options, offsets[1] ?? 0, picked[1], sorting, root);
  const second = useReportLevel(options, offsets[2] ?? 0, picked[2], sorting, first);
  const third = useReportLevel(options, offsets[3] ?? 0, picked[3], sorting, second);
  const levels = [root, first, second, third];
  const primitive = section.id === "primitives";
  const ids = primitive ? ["events", "operations", "hotspots", "stack"] : ["list", "detail", "level-2", "level-3", "level-4"];
  const labels = listLabels[section.id] ?? [section.title];

  function changeOffset(depth, offset) {
    setOffsets(value => Object.assign([...value], { [depth]: offset }));
  }

  function close(depth) {
    setPicked(value => value.slice(0, depth));
    setOffsets(value => value.slice(0, depth + 1));
  }

  function grid(level, depth, label) {
    const sortKeys = depth === 1 && (section.id !== "allocations" || level.parent === "0") ? reportSorts[section.id] : undefined;
    return <>
      <ReportGrid page={level.page} label={label} selected={level.selection?.row} sortKeys={sortKeys}
        serverSort={{ ...sorting, onChange: value => { setSorting(value); changeOffset(depth, 0); } }}
        onSelect={(row, trigger) => {
          if (level.selection?.row !== row) {
            setPicked(value => [...value.slice(0, depth), pickDrillRow(level.page, row, level.scope)]);
            setOffsets(value => value.slice(0, depth + 1));
          }
          revealDetail(trigger, ids[depth + 1]);
        }} />
      <PageControls page={level.page} label={`${label} pages`} onChange={offset => changeOffset(depth, offset)} />
    </>;
  }

  const panes = [{
    id: ids[0], title: labels[0], width: primitive ? 12 : 6,
    content: <section className="runtime-pane" aria-label={labels[0]}>{grid(root, 0, labels[0])}</section>,
  }, ...ids.slice(1).map((id, depth) => {
    const level = levels[depth];
    const chosen = level.selection?.row;
    const next = levels[depth + 1];
    const context = levels[depth - 1]?.selection?.row.title;
    const label = chosen ? `${chosen.title} ${labels[depth + 1] ?? "items"}` : "";
    const selectionTitle = chosen?.title ?? picked[depth]?.title;
    const title = selectionTitle && (!depth || levels[depth - 1].selection)
      && (context ? `${context}: ${selectionTitle}` : selectionTitle);
    return {
      id, title: labels[depth + 1] ?? `${section.title} details`, width: primitive || depth > 0 ? 12 : 6,
      content: title && <Detail id={id} title={title} close={() => close(depth)}>
        {chosen ? <>
          <RowDetails row={chosen} openStack={!chosen.hasChildren} />
          {chosen.hasChildren && next && grid(next, depth + 1, label)}
        </> : level.page.data ? <p className="muted">The selected item is not on this page.</p>
          : <PageStatus page={level.page} label="selection" />}
      </Detail>,
    };
  })];

  return <PaneWorkspace group={section.id} editing={editing} hidden={!active} panes={panes} />;
}

function PageStatus({ page, label }) {
  if (page.error) return <div className="error" role="alert">{page.error.message}
    {page.error.status !== 404 && <button onClick={page.retry}>Retry</button>}
  </div>;
  if (!page.data) return <p className="muted" role="status">Loading {label.toLowerCase()}…</p>;
  return null;
}

function PageControls({ page, label, onChange }) {
  const { data, offset } = page;
  if (!(data?.total > pageSize)) return null;
  const end = offset + data.rows.length;
  return <nav className="actions pagination" aria-label={label}>
    <button disabled={!offset} onClick={() => onChange(offset - pageSize)}>Previous</button>
    <span>{format.format(offset + 1)}–{format.format(end)} of {format.format(data.total)}</span>
    <button disabled={end >= data.total} onClick={() => onChange(offset + pageSize)}>Next</button>
  </nav>;
}

function ReportGrid({ page, label, selected, onSelect, sortKeys, serverSort }) {
  const rows = page.data?.rows ?? [];
  const entries = rows.map((row, index) => ({ row, index, fields: new Map(rowFields(row).map(field => [field.name, field])) }));
  const preferred = ["Events", "Objects", "Contentions", "Occurrences", "State", "Retained", "Lost",
    "Allocated bytes", "Live bytes", "Completed bytes"];
  const names = new Set(entries.flatMap(({ fields }) => [...fields.keys()].filter(Boolean)));
  const fieldNames = sortKeys ? Object.keys(sortKeys) : [...new Set([...preferred.filter(name => names.has(name)), ...names])].slice(0, 3);
  const columns = [
    { name: "Item", value: ({ row }) => row.title },
    ...fieldNames.map(name => ({
      name, sortKey: sortKeys?.[name], value: ({ fields }) => fields.get(name)?.value,
      sortValue: ({ fields }) => {
        const field = fields.get(name);
        return field && ("raw" in field ? field.raw : field.value);
      },
    })),
  ];
  return <>
    <PageStatus page={page} label={label} />
    <DataGrid label={label} rows={entries} columns={columns} rowKey={({ index }) => index}
      selected={selected ? rows.indexOf(selected) : undefined} onSelect={({ row }, trigger) => onSelect(row, trigger)}
      paged={page.data?.total > pageSize} empty={page.data ? "No retained data in this view." : ""} serverSort={serverSort} />
  </>;
}

function RowDetails({ row, stackTitle = "Stack trace", openStack = false }) {
  return <>
    <Fields fields={rowFields(row)} />
    {!!row.stack?.length && <Stack key={row.title} title={stackTitle} frames={row.stack} open={openStack} />}
    {row.relatedStacks?.map(({ title, frames }) => <Stack key={title} title={title} frames={frames} />)}
  </>;
}

function workerPath(row) {
  const { runtimeId, workerId } = row.runtimeWorker;
  return `r/${runtimeId ?? "0"}/${workerId ?? "unbound"}`;
}

function taskId(row) {
  return row.runtimeTask.taskId ?? "0";
}

const workerColumns = [
  { name: "Worker", value: row => row.title },
  { name: "State", value: row => row.runtimeWorker.state || row.runtimeWorker.role },
  { name: "Observed tasks", value: row => formatValue("observedTasks", row.runtimeWorker.observedTasks ?? "0"),
    sortValue: row => row.runtimeWorker.observedTasks ?? "0" },
];
const taskColumns = [
  { name: "Task", value: row => row.title, sortKey: "task" },
  { name: "State", value: ({ runtimeTask: task }) =>
    task.activityState && task.activityState !== "Unknown" ? task.activityState : task.state || "Unknown" },
  { name: "Polls", sortKey: "polls", value: row => formatValue("polls", row.runtimeTask.execution?.polls ?? "0") },
  { name: "Future bytes", sortKey: "future", value: row => formatValue("futureSizeBytes", row.runtimeTask.futureSizeBytes) },
  { name: "Observed execution", sortKey: "executing", value: row => formatValue("executingFraction", row.runtimeTask.execution?.executingFraction) },
  { name: "Median poll (ns)", sortKey: "median", value: row => formatValue("medianPollNanos", row.runtimeTask.execution?.medianPollNanos) },
  { name: "Maximum poll (ns)", sortKey: "maximum", value: row => formatValue("maxPollNanos", row.runtimeTask.execution?.maxPollNanos) },
];
const operationColumns = [
  { name: "Operation", value: row => row.title },
  { name: "Inferred events", value: row => row.fields?.[0]?.value },
];

function RuntimePanel({ snapshot, fullStack, active, editing }) {
  const [pickedWorker, setWorker] = useState(null);
  const [task, setTask] = useState(null);
  const [operation, setOperation] = useState(null);
  const [workerOffset, setWorkerOffset] = useState(0);
  const [taskOffset, setTaskOffset] = useState(0);
  const [operationOffset, setOperationOffset] = useState(0);
  const [occurrenceOffset, setOccurrenceOffset] = useState(0);
  const [occurrence, setOccurrence] = useState(null);
  const [sorting, setSorting] = useState({ by: "task", descending: false });
  const options = { snapshot, section: "runtime", fullStack, by: "task", descending: false };
  const workers = useSnapshotPage({ ...options, fullStack: false, offset: workerOffset, active });
  const onlyWorker = workers.data?.total === 1 ? workers.data.rows?.[0] : null;
  const worker = pickedWorker ?? (onlyWorker ? { path: workerPath(onlyWorker), title: onlyWorker.title } : null);
  const tasks = useSnapshotPage({ ...options, ...sorting, parent: worker?.path, offset: taskOffset, active: active && !!worker });
  const taskPath = worker && task && `${worker.path}/${task}`;
  const detail = useSnapshotPage({ ...options, parent: taskPath, offset: operationOffset, active: active && !!task && !tasks.error });
  const occurrences = useSnapshotPage({
    ...options, parent: `${taskPath}/${operation}`, offset: occurrenceOffset,
    active: active && !!operation && !!detail.data && !tasks.error,
  });
  const selectedTask = detail.data?.subject ?? tasks.data?.rows?.find(row => taskId(row) === task);
  const selectedOccurrence = occurrences.data?.rows?.find(row => occurrenceKey(row) === occurrence);

  function selectWorker(row) {
    const path = workerPath(row);
    if (path === worker?.path) return;
    setWorker({ path, title: row.title });
    setTask(null);
    setOperation(null);
    setOccurrence(null);
    setTaskOffset(0);
  }

  function selectTask(row, trigger) {
    const id = taskId(row);
    if (id === task) return;
    setTask(id);
    setOperation(null);
    setOccurrence(null);
    setOperationOffset(0);
    revealDetail(trigger, "runtime-task");
  }

  return <PaneWorkspace group="runtime" editing={editing} hidden={!active} panes={[
    { id: "workers", title: "Workers", width: 4, wideWidth: 3, content: <section className="runtime-pane" aria-label="Workers">
      <h3>Workers</h3>
      <PageStatus page={workers} label="workers" />
      <DataGrid label="workers" rows={workers.data?.rows ?? []} rowKey={workerPath} selected={worker?.path} onSelect={selectWorker}
        columns={workerColumns} paged={workers.data?.total > pageSize} empty={workers.data ? "No workers in this snapshot." : ""} />
      <PageControls page={workers} label="Worker pages" onChange={setWorkerOffset} />
    </section> },
    { id: "tasks", title: "Tasks", width: 8, wideWidth: 5, content: <section className="runtime-pane" aria-label="Tasks">
      <h3>{worker ? `Tasks · ${tasks.data?.subject?.title ?? worker.title}` : "Tasks"}</h3>
      {!worker ? <p className="muted">Select a worker to see its tasks.</p> : <>
        {tasks.data?.subject && <details className="runtime-worker-details">
          <summary>Worker details</summary>
          <Fields fields={rowFields(tasks.data.subject)} />
          <RuntimeCharts timelines={tasks.data.timelines ?? []} histograms={tasks.data.histograms ?? []} />
        </details>}
        <PageStatus page={tasks} label="tasks" />
        <DataGrid label="tasks" rows={tasks.data?.rows ?? []} rowKey={taskId} selected={task} onSelect={selectTask}
          serverSort={{ ...sorting, onChange: value => { setSorting(value); setTaskOffset(0); } }}
          columns={taskColumns} paged={tasks.data?.total > pageSize} empty={tasks.data ? "No tasks on this worker." : ""} />
        <PageControls page={tasks} label="Task pages" onChange={setTaskOffset} />
      </>}
    </section> },
    { id: "task-detail", title: "Task details", wideWidth: 4, content: task && <Detail id="runtime-task"
      title={`${selectedTask?.title ?? `Task ${task}`} details`} close={() => {
      setTask(null);
      setOperation(null);
      setOccurrence(null);
    }}>
      {tasks.error?.status === 404 ? <p className="muted">Select an available worker to inspect its tasks.</p> : <>
        {selectedTask && !detail.error && <>
          <RowDetails row={selectedTask} stackTitle="Spawn stack" />
          <RuntimeCharts timelines={detail.data?.timelines ?? []} histograms={detail.data?.histograms ?? []} />
        </>}
        <h4>Inferred operations</h4>
        <PageStatus page={detail} label="task details" />
        <DataGrid label="inferred operations" rows={detail.data?.rows ?? []} rowKey={row => row.title} selected={operation}
          columns={operationColumns} paged={detail.data?.total > pageSize} empty={detail.data ? "No inferred operations for this task." : ""}
          onSelect={(row, trigger) => {
            setOperation(row.title);
            setOccurrence(null);
            setOccurrenceOffset(0);
            revealDetail(trigger, "runtime-operation");
          }} />
        <PageControls page={detail} label="Operation pages" onChange={setOperationOffset} />
        {operation && detail.data && <Detail id="runtime-operation" heading="h4" title={`${operation} occurrences`} close={() => {
          setOperation(null);
          setOccurrence(null);
        }}>
          <ReportGrid page={occurrences} label="occurrences" selected={selectedOccurrence} onSelect={(row, trigger) => {
            setOccurrence(occurrenceKey(row));
            revealDetail(trigger, "runtime-occurrence");
          }} />
          <PageControls page={occurrences} label="Occurrence pages" onChange={setOccurrenceOffset} />
          {selectedOccurrence && <Detail id="runtime-occurrence" heading="h5" title={selectedOccurrence.title} close={() => setOccurrence(null)}>
            <RowDetails row={selectedOccurrence} openStack />
          </Detail>}
        </Detail>}
      </>}
    </Detail> },
  ]} />;
}

function occurrenceKey({ taskOccurrence }) {
  return `${taskOccurrence.threadId ?? "0"}:${taskOccurrence.sequence ?? "0"}`;
}

function Stack({ title, frames = [], open }) {
  return frames.length ? <details open={open}><summary>{title} ({frames.length} frames)</summary>
    <ol className="stack">{frames.map((frame, index) => <li key={index}>{frame}</li>)}</ol>
  </details> : <p className="muted">{title}: unavailable</p>;
}

export function Fields({ fields }) {
  return fields.length ? <dl className="fields">{fields.map(({ name, value }, index) => <div key={index}>
    {name && <dt>{name}</dt>}<dd>{value}</dd>
  </div>)}</dl> : null;
}

function RuntimeCharts({ timelines, histograms }) {
  if (!timelines.length && !histograms.length) return null;
  return <div className="runtime-charts">
    {timelines.map(timeline => {
      const start = BigInt(timeline.startNanos ?? 0);
      const duration = Number(BigInt(timeline.endNanos) - start) / 1e9;
      return <figure key={timeline.title}><figcaption>{timeline.title} · {format.format(duration)} s</figcaption>
        <svg viewBox={`0 0 ${timeline.bins.length * 10} 60`} preserveAspectRatio="none" role="img"
          aria-label={`${timeline.title}. Gaps mean no observation.`}>
          {timeline.bins.map(({ fraction }, index) => {
            const height = fraction == null ? 1 : Math.max(2, fraction * 57);
            return <rect key={index} className={fraction == null ? "unknown" : ""} x={index * 10} y={60 - height} width="8" height={height}>
              <title>{fraction == null ? "No observation" : `${(fraction * 100).toFixed(1)}% observed`}</title>
            </rect>;
          })}
        </svg>
      </figure>;
    })}
    {histograms.map(histogram => {
      const max = Math.max(1, ...histogram.buckets.map(bucket => Number(bucket.count ?? 0)));
      return <figure key={histogram.title}><figcaption>{histogram.title}</figcaption>
        <svg viewBox={`0 0 ${histogram.buckets.length * 40} 60`} preserveAspectRatio="none" aria-hidden="true">
          {histogram.buckets.map((bucket, index) => {
            const height = bucket.count ? Math.max(2, Number(bucket.count) * 58 / max) : 1;
            return <rect key={index} className={bucket.count ? "" : "unknown"} x={index * 40} y={60 - height} width="32" height={height} />;
          })}
        </svg>
        <details><summary>Exact bucket counts</summary>
          <Fields fields={histogram.buckets.map(bucket => ({
            name: `${format.format(BigInt(bucket.lowerNanos ?? 0))}–${format.format(BigInt(bucket.upperNanos))} ns`,
            value: format.format(BigInt(bucket.count ?? 0)),
          }))} />
        </details>
      </figure>;
    })}
  </div>;
}
