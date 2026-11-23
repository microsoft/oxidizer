// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

import { useId, useState } from "react";
import { api, download } from "./api.js";
import { DataGrid, Detail, revealDetail } from "./data-grid.jsx";
import { PaneWorkspace } from "./pane-workspace.jsx";
import { CloseButton, TimeRuler, ViewHeading, ViewPanel, ViewTabs } from "./view-chrome.jsx";
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
  workerIds: "Worker IDs", currentTask: "Current task", typeDescriptorId: "Type descriptor ID",
  timestampNanos: "Timestamp (ns)", relativeStackKnown: "Relative stack known",
};
const label = key => labels[key] ?? key.replace(/[A-Z]/g, match => ` ${match.toLowerCase()}`).replace(/^./, c => c.toUpperCase());
const format = new Intl.NumberFormat();
function formatValue(key, value) {
  if (value == null) return "Unknown";
  if (key === "executingFraction") return `${(value * 100).toFixed(1)}%`;
  if (typeof value === "boolean") return value ? "Yes" : "No";
  if (Array.isArray(value)) return value.length ? value.join(", ") : "None";
  if (typeof value === "number" || (typeof value === "string" && /^\d+$/.test(value) &&
      !key.endsWith("Id") && key !== "id" && key !== "currentTask")) {
    return format.format(typeof value === "string" ? BigInt(value) : value);
  }
  return String(value);
}

const models = {
  runtimeWorker: { runtimeId: "0", observedTasks: "0" },
  runtimeTask: { runtimeId: "0", taskId: "0", workerIds: [], repollRequested: false },
  thread: { id: "0", total: "0", retained: "0", lost: "0" },
  allocationHotspot: { allocations: "0", allocatedBytes: "0", liveAllocations: "0", liveBytes: "0" },
  ioResource: { id: "0", reads: "0", writes: "0", completed: "0", errors: "0", canceled: "0", requestedBytes: "0", completedBytes: "0" },
  cacheTier: { id: "0", fallback: false, events: "0", hits: "0", misses: "0", errors: "0" },
  primitiveGroup: { events: "0", objects: "0", contentions: "0" },
  taskOccurrence: { objectId: "0", threadId: "0", sequence: "0", timestampNanos: "0", relativeStackKnown: false },
};

function rowFields(row) {
  for (const [type, defaults] of Object.entries(models)) {
    const model = row[type];
    if (!model) continue;
    const { execution, ...properties } = model;
    const values = { ...defaults, ...properties };
    if (type === "runtimeTask") {
      values.state = taskState(model);
      delete values.activityState;
    }
    if (execution) {
      Object.assign(values, { polls: "0" }, execution);
    }
    return [
      ...Object.entries(values).filter(([, value]) => value != null)
        .map(([key, value]) => ({ name: label(key), value: formatValue(key, value), raw: value })),
      ...(row.fields ?? []),
    ];
  }
  return row.fields ?? [];
}

export function Snapshot({ snapshot, label, source, onLive, active, appliedFilter, onFilter, onClose, action, busy }) {
  const tabId = useId();
  const [section, setSection] = useState("overview");
  const [fullStack, setFullStack] = useState(false);
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
    <ViewHeading kind={source ? null : "Snapshot"} title={label} filename={snapshot.name} context={source ?? "Opened file"}>
      <div className="actions">
        {onLive && <button type="button" className="quiet" disabled={!!busy} onClick={onLive}>Live view</button>}
        <button disabled={!!busy} title={snapshot.savedPath ? `Saved to ${snapshot.savedPath}` : undefined}
          onClick={() => action("Downloading snapshot", () => download(snapshot.id, snapshot.name))}>Download</button>
        <CloseButton disabled={!!busy} onClick={onClose} label="Close snapshot" />
      </div>
    </ViewHeading>
    <div className="snapshot-toolbar">
      <ViewTabs id={tabId} label="Snapshot sections" items={snapshot.sections} selected={activeSection.id} onSelect={setSection} />
      <div className="actions">
        <button className="quiet" aria-expanded={showFilter} onClick={() => setShowFilter(value => !value)}>Stack filter</button>
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
    {snapshot.sections.map(item => {
      const selected = activeSection.id === item.id;
      const props = { snapshot, active: active && selected, fullStack };
      return <ViewPanel key={item.id} id={tabId} item={item.id} selected={activeSection.id}>
        {item.id === "overview" ? <SnapshotOverview {...props} />
          : item.id === "runtime" ? <RuntimePanel {...props} />
          : <SnapshotSection {...props} section={item} />}
      </ViewPanel>;
    })}
  </section>;
}

function SnapshotOverview({ snapshot, active }) {
  const page = useSnapshotPage({ snapshot, section: "overview", offset: 0, fullStack: false, by: "", descending: false, active });
  return <div hidden={!active}>
      <PageStatus page={page} label="Overview" />
      <div className="overview-stats">{page.data?.rows?.map(row => <section key={row.title} className="surface" aria-label={row.title}>
        <h3>{row.title}</h3><Fields fields={rowFields(row)} />
      </section>)}</div>
  </div>;
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

function SnapshotSection({ snapshot, section, active, fullStack }) {
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
        controls={<PageControls page={level.page} label={`${label} pages`} onChange={offset => changeOffset(depth, offset)} />}
        serverSort={{ ...sorting, onChange: value => { setSorting(value); changeOffset(depth, 0); } }}
        onSelect={(row, trigger) => {
          if (level.selection?.row !== row) {
            setPicked(value => [...value.slice(0, depth), pickDrillRow(level.page, row, level.scope)]);
            setOffsets(value => value.slice(0, depth + 1));
          }
          revealDetail(trigger, ids[depth + 1]);
        }} />
    </>;
  }

  const panes = [{
    id: ids[0], title: labels[0],
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
      id, title: primitive ? ["Operations", "Hotspots", "Stack"][depth] : `${section.title} details`,
      content: title && <Detail id={id} title={title} close={() => close(depth)}>
        {chosen ? <>
          <RowDetails row={chosen} openStack={!chosen.hasChildren} />
          {chosen.hasChildren && next && grid(next, depth + 1, label)}
        </> : level.page.data ? <p className="muted">The selected item is not on this page.</p>
          : <PageStatus page={level.page} label="selection" />}
      </Detail>,
    };
  })];

  return <PaneWorkspace group={section.id} hidden={!active} panes={panes} />;
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

function ReportGrid({ page, label, selected, onSelect, sortKeys, serverSort, controls }) {
  const rows = page.data?.rows ?? [];
  const entries = rows.map((row, index) => ({ row, index, fields: new Map(rowFields(row).map(field => [field.name, field])) }));
  const preferred = ["Events", "Objects", "Contentions", "Occurrences", "State", "Retained", "Lost",
    "Allocated bytes", "Live bytes", "Completed bytes"];
  const names = new Set(entries.flatMap(({ fields }) => [...fields.keys()].filter(Boolean)));
  const fieldNames = sortKeys ? Object.keys(sortKeys) : [...new Set([...preferred.filter(name => names.has(name)), ...names])].slice(0, 3);
  const columns = [
    { name: "Item", value: ({ row }) => row.title },
    ...fieldNames.map(name => ({
      name, numeric: entries.some(({ fields }) => /^-?\d[\d.,]*%?$/.test(fields.get(name)?.value ?? "")),
      sortKey: sortKeys?.[name], value: ({ fields }) => fields.get(name)?.value,
      sortValue: ({ fields }) => {
        const field = fields.get(name);
        return field && ("raw" in field ? field.raw : field.value);
      },
    })),
  ];
  return <DataGrid label={label} rows={entries} columns={columns} rowKey={({ index }) => index}
      controls={page.data?.total > pageSize ? controls : null}
      status={page.data ? undefined : { loading: !page.error, content: <PageStatus page={page} label={label} /> }}
      selected={selected ? rows.indexOf(selected) : undefined} onSelect={({ row }, trigger) => onSelect(row, trigger)}
      paged={page.data?.total > pageSize} empty={page.data ? "No retained data in this view." : ""} serverSort={serverSort} />;
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
  { name: "Observed tasks", numeric: true, value: row => formatValue("observedTasks", row.runtimeWorker.observedTasks ?? "0"),
    sortValue: row => row.runtimeWorker.observedTasks ?? "0" },
];
const taskViews = [{ id: "operations", title: "Operations" }, { id: "timing", title: "Timing" }, { id: "metadata", title: "Metadata" }];
const taskState = task => task.activityState && task.activityState !== "Unknown" ? task.activityState : task.state;

const taskColumns = [
  { name: "Task", value: row => row.title, sortKey: "task" },
  { name: "State", value: row => taskState(row.runtimeTask) },
  { name: "Polls", numeric: true, sortKey: "polls", value: row => formatValue("polls", row.runtimeTask.execution?.polls ?? "0") },
  { name: "Future bytes", numeric: true, sortKey: "future", value: row => row.runtimeTask.futureSizeBytes == null ? null : formatValue("futureSizeBytes", row.runtimeTask.futureSizeBytes) },
  { name: "Execution", numeric: true, sortKey: "executing", value: row => row.runtimeTask.execution?.executingFraction == null ? null : formatValue("executingFraction", row.runtimeTask.execution.executingFraction) },
  { name: "Median (ns)", numeric: true, sortKey: "median", value: row => row.runtimeTask.execution?.medianPollNanos == null ? null : formatValue("medianPollNanos", row.runtimeTask.execution.medianPollNanos) },
  { name: "Max (ns)", numeric: true, sortKey: "maximum", value: row => row.runtimeTask.execution?.maxPollNanos == null ? null : formatValue("maxPollNanos", row.runtimeTask.execution.maxPollNanos) },
];
const operationColumns = [
  { name: "Operation", value: row => row.title },
  { name: "Inferred events", numeric: true, value: row => row.fields?.find(field => field.name === "Inferred events")?.value },
];

function RuntimePanel({ snapshot, fullStack, active }) {
  const taskTabsId = useId();
  const [pickedWorker, setWorker] = useState(null);
  const [task, setTask] = useState(null);
  const [taskView, setTaskView] = useState("operations");
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

  function selectWorker(row, trigger) {
    revealDetail(trigger, "runtime-tasks");
    const path = workerPath(row);
    if (path === worker?.path) return;
    setWorker({ path, title: row.title });
    setTask(null);
    setOperation(null);
    setOccurrence(null);
    setTaskOffset(0);
  }

  function selectTask(row, trigger) {
    revealDetail(trigger, "runtime-task");
    const id = taskId(row);
    if (id === task) return;
    setTask(id);
    setTaskView("operations");
    setOperation(null);
    setOccurrence(null);
    setOperationOffset(0);
  }

  return <PaneWorkspace group="runtime" hidden={!active} panes={[
    { id: "workers", title: "Workers", content: <section className="runtime-pane" aria-label="Workers">
      <PageStatus page={workers} label="workers" />
      <DataGrid label="workers" rows={workers.data?.rows ?? []} rowKey={workerPath} selected={worker?.path} onSelect={selectWorker}
        columns={workerColumns} paged={workers.data?.total > pageSize} empty={workers.data ? "No workers in this snapshot." : ""} />
      <PageControls page={workers} label="Worker pages" onChange={setWorkerOffset} />
    </section> },
    { id: "tasks", title: "Tasks", content: <section className="runtime-pane" aria-label="Tasks" data-inspection="runtime-tasks">
      {worker && <h3>{tasks.data?.subject?.title ?? worker.title}</h3>}
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
    { id: "task-detail", title: "Task details", content: task && <Detail id="runtime-task"
      title={`${selectedTask?.title ?? `Task ${task}`} details`} close={() => {
      setTask(null);
      setOperation(null);
      setOccurrence(null);
    }}>
      {tasks.error?.status === 404 ? <p className="muted">Select an available worker to inspect its tasks.</p> : <>
        {selectedTask && <TaskSummary task={selectedTask.runtimeTask} />}
        <ViewTabs id={taskTabsId} label="Task details" items={taskViews} selected={taskView} onSelect={setTaskView} inspector />
        <ViewPanel id={taskTabsId} item="operations" selected={taskView}>
        <DataGrid label="inferred operations" rows={detail.data?.rows ?? []} rowKey={row => row.title} selected={operation}
          controls={detail.data?.total > pageSize ? <PageControls page={detail} label="Operation pages" onChange={setOperationOffset} /> : null}
          status={detail.data ? undefined : { loading: !detail.error, content: <PageStatus page={detail} label="inferred operations" /> }}
          columns={operationColumns} paged={detail.data?.total > pageSize} empty={detail.data ? "No inferred operations for this task." : ""}
          onSelect={(row, trigger) => {
            setOperation(row.title);
            setOccurrence(null);
            setOccurrenceOffset(0);
            revealDetail(trigger, "runtime-operation");
          }} />
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
        </ViewPanel>
        <ViewPanel id={taskTabsId} item="timing" selected={taskView}>
          <PageStatus page={detail} label="timing" />
          {detail.data && ((detail.data.timelines?.length || detail.data.histograms?.length)
            ? <RuntimeCharts timelines={detail.data.timelines ?? []} histograms={detail.data.histograms ?? []} />
            : <p className="muted">No timing data for this task.</p>)}
        </ViewPanel>
        <ViewPanel id={taskTabsId} item="metadata" selected={taskView}>
          {selectedTask ? <RowDetails row={selectedTask} stackTitle="Spawn stack" openStack /> : <PageStatus page={detail} label="metadata" />}
        </ViewPanel>
      </>}
    </Detail> },
  ]} />;
}

function TaskSummary({ task }) {
  return <dl className="stats task-summary">
    <div><dt>State</dt><dd>{taskState(task) ?? "Unknown"}</dd></div>
    <div><dt>Polls</dt><dd>{task.execution ? formatValue("polls", task.execution.polls ?? "0") : "Unknown"}</dd></div>
    <div><dt>Observed execution</dt><dd>{formatValue("executingFraction", task.execution?.executingFraction)}</dd></div>
  </dl>;
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
  // The report API uses the same observation window for every timeline in a page.
  const observation = timelines[0];
  return <div className="runtime-charts">
    {observation && <section className="timing-tracks" aria-label="Timing tracks">
      <div className="timing-track"><span>Elapsed time</span>
        <TimeRuler seconds={Number(BigInt(observation.endNanos) - BigInt(observation.startNanos ?? 0)) / 1e9} />
      </div>
      {timelines.map(timeline => <figure className="timing-track" key={timeline.title}><figcaption>{timeline.title}</figcaption>
        <svg className="track-plot" viewBox={`0 0 ${timeline.bins.length * 10} 60`} preserveAspectRatio="none" role="img"
          aria-label={`${timeline.title}. Gray marks mean no observation.`}>
          {timeline.bins.map(({ fraction }, index) => {
            const height = fraction == null ? 3 : Math.max(2, fraction * 57);
            return <rect key={index} className={fraction == null ? "unknown" : fraction === 0 ? "zero" : undefined}
              x={index * 10} y={60 - height} width="9" height={height}>
              <title>{fraction == null ? "No observation"
                : `${(fraction * 100).toFixed(1)}% ${timeline.title.startsWith("Ready intervals") ? "ready" : "observed"}`}</title>
            </rect>;
          })}
        </svg>
      </figure>)}
      {timelines.some(timeline => timeline.bins.some(bin => bin.fraction == null)) &&
        <p className="timing-track-note">Gray marks indicate no observation.</p>}
    </section>}
    {!!histograms.length && <div className="histograms">{histograms.map(histogram => {
      const counts = histogram.buckets.map(bucket => BigInt(bucket.count ?? 0));
      const max = Math.max(1, ...counts.map(Number));
      const range = bucket => {
        const start = format.format(BigInt(bucket.lowerNanos ?? 0));
        return bucket.upperNanos === "18446744073709551615"
          ? `At least ${start} ns`
          : `${start}–${format.format(BigInt(bucket.upperNanos))} ns`;
      };
      return <figure key={histogram.title}><figcaption>{histogram.title}</figcaption>
        <svg viewBox={`0 0 ${histogram.buckets.length * 40} 60`} preserveAspectRatio="none" aria-hidden="true">
          {histogram.buckets.map((bucket, index) => {
            const count = counts[index];
            const height = count > 0n ? Math.max(2, Number(count) * 58 / max) : 1;
            return <rect key={index} className={count > 0n ? undefined : "empty-bucket"} x={index * 40} y={60 - height} width="32" height={height}>
              <title>{range(bucket)}: {format.format(count)}</title>
            </rect>;
          })}
        </svg>
        <details><summary>Exact bucket counts</summary>
          <Fields fields={histogram.buckets.filter((_, index) => counts[index] > 0n).map(bucket => ({
            name: range(bucket),
            value: format.format(BigInt(bucket.count ?? 0)),
          }))} />
        </details>
      </figure>;
    })}</div>}
  </div>;
}
