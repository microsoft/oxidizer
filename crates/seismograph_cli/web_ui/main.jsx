// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import { rate, sample } from "./activity.js";
import { api, token, upload } from "./api.js";
import { Fields, Snapshot } from "./snapshot.jsx";
import { addSnapshot, closeSnapshot } from "./snapshot-state.js";
import { DataGrid, Detail, revealDetail } from "./data-grid.jsx";
import { PaneLayoutProvider } from "./pane-workspace.jsx";
import "./monitor.css";

const groups = [
  ["allocations", "Allocations"], ["generalEvents", "Primitives"], ["arcDereferences", "Arc dereferences"],
  ["runtimeTasks", "Runtime tasks"], ["io", "I/O"], ["cache", "Cache"],
];
const format = new Intl.NumberFormat();
const count = value => format.format(BigInt(value ?? 0));
const amount = value => `${format.format(Number(value ?? 0) / 1024 ** 2)} MiB`;
const displayName = app => app.instance ? `${app.name} (${app.instance})` : app.name;
const recording = value => groups.some(([key]) => value?.[key]?.enabled);

function usePoll(load, interval, generation = 0, update = withResponse) {
  const [result, setResult] = useState({});
  useEffect(() => {
    if (!load) return;
    const controller = new AbortController();
    let timer;
    async function read() {
      try {
        const data = await load(controller.signal);
        const now = performance.now();
        if (!controller.signal.aborted) setResult(previous => update(data, previous, now));
      } catch (error) {
        if (!controller.signal.aborted) setResult(previous => ({ ...previous, error: error.message }));
      } finally {
        if (!controller.signal.aborted) timer = setTimeout(read, interval);
      }
    }
    read();
    return () => { controller.abort(); clearTimeout(timer); };
  }, [load, interval, generation, update]);
  return result;
}

function withResponse(data) { return { data }; }

function App() {
  const [selected, setSelected] = useState(null);
  const [workspace, setWorkspace] = useState({ tabs: [], active: null });
  const [snapshotView, setSnapshotView] = useState(false);
  const capture = workspace.tabs.find(tab => tab.snapshot.id === workspace.active);
  const showingSnapshot = snapshotView && !!capture;
  const [refresh, setRefresh] = useState(0);
  const [busy, setBusy] = useState("");
  const [error, setError] = useState("");
  const running = useRef(false);
  const file = useRef(null);
  const main = useRef(null);
  const positions = useRef(new Map());
  const restoreFocus = useRef(false);
  const found = usePoll(token ? api.discover : null, 3000, refresh);
  const apps = found.data?.applications ?? [];
  const current = apps.find(app => app.id === selected);
  const viewKey = showingSnapshot ? `snapshot:${capture.viewKey}` : selected ? `live:${selected}` : "applications";

  function rememberView(focusHeading = false) {
    positions.current.set(viewKey, main.current.scrollTop);
    restoreFocus.current = focusHeading;
  }

  useLayoutEffect(() => {
    if (!main.current) return;
    main.current.scrollTop = positions.current.get(viewKey) ?? 0;
    if (restoreFocus.current) {
      (main.current.querySelector("section:not([hidden]) .view-title") ?? main.current).focus({ preventScroll: true });
      restoreFocus.current = false;
    }
  }, [viewKey]);

  async function action(label, run) {
    if (running.current) return false;
    running.current = true;
    setBusy(label);
    setError("");
    try { await run(); return true; }
    catch (error) { setError(error.message); return false; }
    finally { running.current = false; setBusy(""); }
  }

  function readFile(event) {
    const uploadFile = event.target.files[0];
    event.target.value = "";
    if (uploadFile) void action("Opening snapshot", async () => {
      const snapshot = await upload(uploadFile);
      rememberView(true);
      setWorkspace(value => addSnapshot(value, snapshot, null));
      setSnapshotView(true);
    });
  }

  async function showCapture(appId, snapshot) {
    const filter = capture?.source === appId ? capture.filter : null;
    if (filter) {
      try {
        snapshot = await api.filter(snapshot.id, filter);
      } catch (error) {
        rememberView(true);
        setWorkspace(value => addSnapshot(value, snapshot, appId, filter));
        setSnapshotView(true);
        throw new Error(`The snapshot opened without its stack filter. ${error.message}`, { cause: error });
      }
    }
    rememberView(true);
    setWorkspace(value => addSnapshot(value, snapshot, appId, filter));
    setSnapshotView(true);
  }

  function updateFilter(snapshot, filter) {
    setWorkspace(value => ({ ...value, tabs: value.tabs.map(tab => tab.snapshot.id === snapshot.id
      ? { ...tab, snapshot, filter: filter.include.trim() || filter.exclude.trim() ? filter : null } : tab) }));
  }

  function selectSnapshot(id) {
    if (showingSnapshot && id === workspace.active) return;
    rememberView();
    setWorkspace(value => ({ ...value, active: id }));
    setSnapshotView(true);
  }

  function selectApplication(id, focusHeading = false) {
    if (!showingSnapshot && id === selected) return;
    rememberView(focusHeading);
    setSelected(id);
    setSnapshotView(false);
  }

  return <>
    <a className="skip" href="#workspace">Skip to content</a>
    <header className="masthead">
      <a className="brand" href="/" onClick={event => {
        event.preventDefault();
        if (!running.current) selectApplication(null);
      }}><svg viewBox="0 0 32 32" width="32" height="32" aria-hidden="true"><path d="M2 17h6l4-9 5 17 4-12 3 4h6" /></svg>
        <span>Seismograph<small>PROCESS MONITOR</small>        </span></a>
      <span className="connection"><i className={found.data && !found.error ? "online" : ""} /> Local proxy
        {found.data?.monitorVersion && <span className="muted"> · v{found.data.monitorVersion}</span>}</span>
      <button disabled={!token || !!busy} onClick={() => file.current.click()}>Open snapshot</button>
      <input ref={file} type="file" accept=".seismograph" hidden onChange={readFile} />
    </header>
    {!!workspace.tabs.length && <nav className="capture-tabs" aria-label="Snapshot tabs">
      <span>Snapshots</span>
      {workspace.tabs.map(({ snapshot }) => <button key={snapshot.id} disabled={!!busy}
        title={snapshot.name} aria-current={showingSnapshot && snapshot.id === workspace.active ? "page" : undefined}
        onClick={() => selectSnapshot(snapshot.id)}>{snapshot.name}</button>)}
    </nav>}
    {!token ? <main id="workspace" className="locked">
      <h1>Connect to the local proxy</h1>
      <p>Run <code>seismograph monitor-web</code> and open the full URL from the terminal.</p>
    </main> : <div className="layout">
      <aside className="sidebar">
        <div className="sidebar-top"><h2>Applications <span>{apps.length}</span></h2>
          <button onClick={() => setRefresh(value => value + 1)} aria-label="Refresh applications" title="Refresh applications">↻</button></div>
        {found.error && <p className="error" role="alert">{found.error}</p>}
        {!found.data && !found.error && <p className="muted" role="status">Finding applications…</p>}
        <DataGrid label="applications" rows={apps} rowKey={app => app.id} selected={showingSnapshot ? null : selected}
          onSelect={app => selectApplication(app.id)}
          columns={[{ name: "Application", value: displayName }, { name: "PID", value: app => app.pid }]}
          disabled={!!busy} empty={found.data ? "No applications found. Start an application with a Seismograph monitor." : ""} />
        {!!found.data?.warnings?.length && <details><summary>Discovery warnings ({found.data.warnings.length})</summary>
          <ul>{found.data.warnings.map((warning, index) => <li key={index}>{warning}</li>)}</ul></details>}
      </aside>
      <main id="workspace" ref={main} tabIndex={-1}>
        {error && <div className="error notice" role="alert">{error}<button onClick={() => setError("")}>Dismiss</button></div>}
        {busy && <p className="working" role="status">{busy}…</p>}
        {!showingSnapshot && selected && !current && !found.error && <p className="notice" role="status">The selected application is no longer available.</p>}
        {current && <Live key={current.id} app={current} hidden={showingSnapshot} busy={busy} action={action}
          onCapture={showCapture} disconnect={() => selectApplication(null, true)} />}
        {workspace.tabs.map(tab => <Snapshot key={tab.viewKey} snapshot={tab.snapshot} active={showingSnapshot && tab === capture}
          appliedFilter={tab.filter} onFilter={updateFilter} onClose={() => {
            rememberView(true);
            setWorkspace(value => closeSnapshot(value, tab.snapshot.id));
          }} action={action} busy={busy} />)}
        {!showingSnapshot && !current && <div className="welcome">
          <h1 className="view-title" tabIndex={-1}>Inspect a running application</h1>
          <p>Select an application to see recorder activity, change recording policies, and capture a snapshot.</p>
          <button disabled={!!busy} onClick={() => file.current.click()}>Open a saved snapshot</button>
        </div>}
      </main>
    </div>}
  </>;
}

function Live({ app, action, busy, onCapture, disconnect, hidden }) {
  const [load] = useState(() => signal => api.activity(app.id, signal));
  const [generation, setGeneration] = useState(0);
  const { data, past, time, pastTime, error, history } = usePoll(load, 1000, generation, sample);
  const [editing, setEditing] = useState(false);
  const [showThreads, setShowThreads] = useState(false);
  const [threadId, setThreadId] = useState(null);
  const config = data?.recording ?? app.recording;
  const threads = data?.threads ?? [];
  const priorThreads = new Map((past?.threads ?? []).map(thread => [thread.id, thread]));
  const threadRate = thread => {
    const prior = priorThreads.get(thread.id);
    return !error && prior ? rate(thread.totalEvents, prior.totalEvents, time - pastTime) : null;
  };
  const threadName = thread => `${thread.name || "Unnamed"} (${thread.id})`;
  const threadColumns = [
    { name: "Thread", value: threadName },
    { name: "State", value: thread => thread.retired ? "Exited" : "Active" },
    { name: "Events/s", value: thread => threadRate(thread) == null ? "—" : count(threadRate(thread)), sortValue: threadRate },
    { name: "Retained", value: thread => count(thread.retainedEvents), sortValue: thread => thread.retainedEvents ?? "0" },
    { name: "Lost", value: thread => count(thread.lostEvents), sortValue: thread => thread.lostEvents ?? "0" },
  ];
  const selectedThread = threads.find(thread => thread.id === threadId);

  async function capture(mode) {
    const warnings = { CLEAR: "Capture and clear retained events?", STOP: "Capture, stop recording, and release the event buffers?" };
    if (warnings[mode] && !window.confirm(warnings[mode])) return;
    await action("Capturing snapshot", async () => {
      try { await onCapture(app.id, await api.capture(app.id, mode)); }
      finally { setGeneration(value => value + 1); }
    });
  }

  async function clear() {
    if (!window.confirm("Clear retained events without saving a snapshot?")) return;
    await action("Clearing event buffers", async () => {
      try { await api.clear(app.id); }
      finally { setGeneration(value => value + 1); }
    });
  }

  return <section className="live" aria-label="Live application" hidden={hidden}>
    <div className="page-heading"><div><h1 className="view-title" tabIndex={-1}>{displayName(app)}</h1>
      <p className="muted">PID {app.pid} · {app.owner} · Seismograph {app.serverVersion || "unknown"}</p></div>
      <button disabled={!!busy} onClick={disconnect}>Disconnect</button>
    </div>
    {error && <p className="error" role="alert">Live activity is not current. {error}</p>}
    <div className="stats">
      {[
        ["State", recording(config) ? "Recording" : "Off"], ["Retained events", data && count(data.retainedEvents)],
        ["Lost events", data && count(data.lostEvents)], ["Recorder memory", data && amount(data.allocatedBytes)],
        ["Threads", data && count(data.threadCount)],
      ].map(([label, value]) => <div key={label}><span>{label}</span><strong>{value ?? "—"}</strong></div>)}
    </div>
    <div className="actions">
      <button className="primary" disabled={!!busy} onClick={() => capture("CONTINUE")}>Capture snapshot</button>
      <button disabled={!!busy} onClick={() => capture("STOP")}>Capture and stop</button>
      <button disabled={!!busy} onClick={() => capture("CLEAR")}>Capture and clear</button>
      <button disabled={!!busy || !config} onClick={() => setEditing(value => !value)} aria-expanded={editing}>Recording policies</button>
      <button disabled={!!busy} onClick={clear}>Clear buffers</button>
    </div>
    {editing && <Policies initial={config} busy={busy} close={() => setEditing(false)} save={async value => {
      const saved = await action("Saving recording policies", async () => {
        await api.record(app.id, value);
        setGeneration(value => value + 1);
      });
      if (saved) setEditing(false);
    }} />}
    <div className="surface activity">
      <div className="section-heading"><h2>Live activity</h2>
        <button onClick={() => setShowThreads(value => !value)} aria-expanded={showThreads}>
          {showThreads ? "Hide" : "Show"} threads ({count(threads.length)})
        </button></div>
      <ActivityChart history={history ?? []} stale={!!error} />
      <div className="rates">
        {groups.map(([key, title], index) => {
          const value = error ? null : history?.at(-1)?.rates[index];
          return <div key={key}><span className={`event-series series-${index}`}>{title}</span>
            <strong>{value == null ? "—" : `${count(value)} /s`}</strong>
            {data?.classEvents && <small>{count(data.classEvents[index])} total</small>}</div>;
        })}
      </div>
      <div className={`detail-layout${selectedThread ? " with-detail" : ""}`} hidden={!showThreads}>
        <DataGrid label="live threads" rows={threads} rowKey={thread => thread.id} selected={threadId}
          columns={threadColumns} onSelect={(thread, trigger) => {
            setThreadId(thread.id);
            revealDetail(trigger, "live-thread");
          }} empty="No recorded threads." />
        {selectedThread && <Detail id="live-thread" title={threadName(selectedThread)} close={() => setThreadId(null)}>
          <Fields fields={[
            ...threadColumns.slice(1).map(column => ({ name: column.name, value: column.value(selectedThread) })),
            { name: "Total events", value: count(selectedThread.totalEvents) },
            { name: "Capacity", value: count(selectedThread.capacity) },
          ]} />
        </Detail>}
      </div>
    </div>
  </section>;
}

function ActivityChart({ history, stale }) {
  if (history.length < 2) return null;
  const start = history[0].time;
  const span = history.at(-1).time - start;
  const ceiling = Math.max(1, ...history.flatMap(point => point.rates));
  return <figure className={stale ? "activity-chart stale" : "activity-chart"}>
    <figcaption>{stale ? "Last available history" : "Recorder activity history"} · {count(ceiling)} events/s max · {Math.round(span / 1000)} s</figcaption>
    <svg viewBox="0 0 640 96" preserveAspectRatio="none" aria-hidden="true">
      <path className="grid-lines" d="M0 1H640 M0 48H640 M0 95H640" />
      {groups.map(([key], index) => {
        let penDown = false;
        const line = history.map(point => {
          const value = point.rates[index];
          if (value == null) { penDown = false; return ""; }
          const command = penDown ? "L" : "M";
          penDown = true;
          return `${command}${(640 * (point.time - start) / span).toFixed(1)} ${(95 - 94 * value / ceiling).toFixed(1)}`;
        }).join(" ");
        return <path key={key} className={`series-${index}`} d={line} />;
      })}
    </svg>
  </figure>;
}

function Policies({ initial, save, close, busy }) {
  const [config, setConfig] = useState(initial);
  const update = (key, name, value) => setConfig(previous => ({
    ...previous, [key]: { ...previous[key], [name]: value },
  }));
  function preset(enabled) {
    setConfig(previous => ({
      ...previous,
      ...Object.fromEntries(groups.map(([key]) => [key, { enabled, captureBacktraces: enabled, samplingOneIn: 1 }])),
    }));
  }
  return <form className="surface" onSubmit={event => { event.preventDefault(); void save(config); }}>
    <div className="section-heading"><h2>Recording policies</h2><div className="actions">
      <button type="button" disabled={!!busy} onClick={() => preset(true)}>Enable all</button>
      <button type="button" disabled={!!busy} onClick={() => preset(false)}>Disable all</button></div></div>
    <div className="table-scroll"><table><thead><tr><th>Class</th><th>Enabled</th><th>Backtraces</th><th>Sample 1 in</th></tr></thead>
      <tbody>{groups.map(([key, label]) => <tr key={key}><th scope="row">{label}</th>
        <td><input aria-label={`${label} enabled`} type="checkbox" checked={!!config[key]?.enabled} onChange={event => update(key, "enabled", event.target.checked)} /></td>
        <td><input aria-label={`${label} backtraces`} type="checkbox" checked={!!config[key]?.captureBacktraces} onChange={event => update(key, "captureBacktraces", event.target.checked)} /></td>
        <td><input aria-label={`${label} sampling denominator`} type="number" required min="1" max="1048576" value={config[key]?.samplingOneIn ?? ""} onChange={event => update(key, "samplingOneIn", event.target.valueAsNumber || "")} /></td>
      </tr>)}</tbody></table></div>
    <div className="actions"><label>Events per thread <select value={config.eventCapacityPerThread}
      onChange={event => setConfig({ ...config, eventCapacityPerThread: Number(event.target.value) })}>
      {Array.from({ length: 15 }, (_, index) => 2 ** (index + 6)).map(capacity =>
        <option key={capacity} value={capacity}>{count(capacity)}</option>)}
    </select></label>
      <button className="primary" disabled={!!busy}>Apply policies</button><button type="button" onClick={close}>Cancel</button></div>
  </form>;
}

createRoot(document.getElementById("root")).render(<PaneLayoutProvider><App /></PaneLayoutProvider>);
