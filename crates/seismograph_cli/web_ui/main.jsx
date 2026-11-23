// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

import { useCallback, useEffect, useId, useLayoutEffect, useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import { rate, sample } from "./activity.js";
import { api, isAccessKey, setAccessKey, token, upload } from "./api.js";
import { Snapshot } from "./snapshot.jsx";
import { addSnapshot, captureFor, closeSnapshot, snapshotGroups } from "./snapshot-state.js";
import { DataGrid } from "./data-grid.jsx";
import { MonitorLayout } from "./monitor-layout.jsx";
import { DetailCard, TimeRuler, ViewHeading, ViewPanel, ViewTabs } from "./view-chrome.jsx";
import "./monitor.css";
import "./dock-theme.css";

const groups = [
  ["allocations", "Allocations"], ["generalEvents", "Primitives"], ["arcDereferences", "Arc dereferences"],
  ["runtimeTasks", "Runtime tasks"], ["io", "I/O"], ["cache", "Cache"],
];
const liveViews = [{ id: "activity", title: "Activity" }, { id: "threads", title: "Threads" }, { id: "recording", title: "Recording" }];
const format = new Intl.NumberFormat();
const count = value => format.format(BigInt(value ?? 0));
const amount = value => `${format.format(Number(value ?? 0) / 1024 ** 2)} MiB`;
const smallRate = new Intl.NumberFormat(undefined, { maximumFractionDigits: 1 });
const formatRate = value => value > 0 && value < .1 ? "<0.1" : value < 10 ? smallRate.format(value) : format.format(Math.round(value));
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
  const [accessKey, setKey] = useState(() => token);
  const [signInError, setSignInError] = useState("");
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
  const discover = useCallback(async signal => {
    try {
      return await api.discover(signal);
    } catch (error) {
      if (!signal.aborted && error.status === 401) {
        setAccessKey(null);
        setKey(null);
        setSelected(null);
        setWorkspace({ tabs: [], active: null });
        setSnapshotView(false);
        positions.current.clear();
        setSignInError("The access key is not valid. Enter the key from the current terminal.");
      }
      throw error;
    }
  }, []);
  const found = usePoll(accessKey ? discover : null, 3000, refresh);
  const apps = found.data?.applications ?? [];
  const { applications, files } = snapshotGroups(apps, workspace.tabs);
  const current = apps.find(app => app.id === selected);
  const viewKey = showingSnapshot ? `snapshot:${capture.viewKey}` : selected ? `live:${selected}` : "applications";

  function signIn(event) {
    event.preventDefault();
    const key = String(new FormData(event.currentTarget).get("access-key")).trim().toLowerCase();
    if (!isAccessKey(key)) {
      setSignInError("Enter the 64-character access key from the terminal.");
      return;
    }
    setAccessKey(key);
    setSignInError("");
    setKey(key);
  }

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

  async function showCapture(app, snapshot) {
    const filter = captureFor(workspace, app.id)?.filter;
    if (filter) {
      try {
        snapshot = await api.filter(snapshot.id, filter);
      } catch (error) {
        rememberView(true);
        setWorkspace(value => addSnapshot(value, snapshot, app));
        setSnapshotView(true);
        throw new Error(`The snapshot opened without its stack filter. ${error.message}`, { cause: error });
      }
    }
    rememberView(true);
    setWorkspace(value => addSnapshot(value, snapshot, app, filter));
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

  function snapshotList(tabs, label) {
    if (!tabs.length) return null;
    return <ul className="snapshot-links" aria-label={label}>
      {tabs.map(({ snapshot, label }) => <li key={snapshot.id}>
        <button type="button" title={snapshot.name} disabled={!!busy}
          aria-current={showingSnapshot && snapshot.id === workspace.active ? "page" : undefined}
          onClick={() => selectSnapshot(snapshot.id)}>{label}</button>
      </li>)}
    </ul>;
  }

  return <>
    <a className="skip" href="#workspace">Skip to content</a>
    <header className="masthead">
      <a className="brand" href="/" onClick={event => {
        event.preventDefault();
        if (accessKey && !running.current) selectApplication(null);
      }}><svg viewBox="0 0 32 32" width="32" height="32" aria-hidden="true"><path d="M2 17h6l4-9 5 17 4-12 3 4h6" /></svg>
        <span>Seismograph</span></a>
      <span className="connection"><span className={`connection-dot${found.data && !found.error ? " online" : ""}`} aria-hidden="true" />
        {!accessKey ? "Not connected" : found.error ? "Proxy unavailable" : found.data ? "Connected" : "Connecting"}
        {found.data?.monitorVersion && <span className="muted">v{found.data.monitorVersion}</span>}</span>
      <button disabled={!accessKey || !!busy} onClick={() => file.current.click()}>Open snapshot</button>
      <input ref={file} type="file" accept=".seismograph" hidden onChange={readFile} />
    </header>
    {!accessKey ? <main id="workspace" className="locked">
      <h1>Connect to the local proxy</h1>
      <p>Enter the access key from the terminal.</p>
      <form className="actions" onSubmit={signIn}>
        <label>Access key <input name="access-key" type="password" autoComplete="off" required /></label>
        <button className="primary">Connect</button>
      </form>
      {signInError && <p className="error" role="alert">{signInError}</p>}
    </main> : <MonitorLayout sidebar={
      <aside className="sidebar" id="application-list">
        <div className="sidebar-top"><h2>Applications <span>{apps.length}</span></h2>
          <button className="quiet" onClick={() => setRefresh(value => value + 1)} aria-label="Refresh applications" title="Refresh applications">↻</button></div>
        {found.error && <p className="error" role="alert">{found.error}</p>}
        {!found.data && !found.error && <p className="muted" role="status">Finding applications…</p>}
        <DataGrid label="applications" rows={applications} rowKey={app => app.id} selected={showingSnapshot ? null : selected}
          onSelect={app => selectApplication(app.id)}
          columns={[{ name: "Application", value: displayName }, { name: "PID", numeric: true, value: app => app.online ? app.pid : "Offline" }]}
          canSelectRow={app => app.online}
          renderChildren={app => snapshotList(app.snapshots, `Snapshots for ${displayName(app)}`)}
          disabled={!!busy} empty={found.data ? "No applications found. Start an application with a Seismograph monitor." : ""} />
        {!!files.length && <section className="snapshot-files">
          <h2>Opened files</h2>
          {snapshotList(files, "Opened snapshot files")}
        </section>}
        {!!found.data?.warnings?.length && <details><summary>Discovery warnings ({found.data.warnings.length})</summary>
          <ul>{found.data.warnings.map((warning, index) => <li key={index}>{warning}</li>)}</ul></details>}
      </aside>
    }>
      <main id="workspace" ref={main} tabIndex={-1}>
        {error && <div className="error notice" role="alert">{error}<button onClick={() => setError("")}>Dismiss</button></div>}
        {busy && <p className="working" role="status">{busy}…</p>}
        {!showingSnapshot && selected && !current && !found.error && <p className="notice" role="status">The selected application is no longer available.</p>}
        {current && <Live key={current.id} app={current} hidden={showingSnapshot} busy={busy} action={action}
          onCapture={showCapture} disconnect={() => selectApplication(null, true)} />}
        {workspace.tabs.map(tab => <Snapshot key={tab.viewKey} snapshot={tab.snapshot} label={tab.label}
          source={tab.application ? displayName(tab.application) : null}
          onLive={apps.some(app => app.id === tab.source) ? () => selectApplication(tab.source, true) : undefined}
          active={showingSnapshot && tab === capture}
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
    </MonitorLayout>}
  </>;
}

function Live({ app, action, busy, onCapture, disconnect, hidden }) {
  const tabId = useId();
  const [load] = useState(() => signal => api.activity(app.id, signal));
  const [generation, setGeneration] = useState(0);
  const { data, past, time, pastTime, error, history } = usePoll(load, 1000, generation, sample);
  const [section, setSection] = useState("activity");
  const [editing, setEditing] = useState(false);
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
    { name: "Events/s", numeric: true, value: thread => {
      const value = threadRate(thread);
      return value == null ? null : formatRate(value);
    }, sortValue: threadRate },
    { name: "Retained", numeric: true, value: thread => count(thread.retainedEvents), sortValue: thread => thread.retainedEvents ?? "0" },
    { name: "Lost", numeric: true, value: thread => count(thread.lostEvents), sortValue: thread => thread.lostEvents ?? "0" },
    { name: "Total events", numeric: true, value: thread => count(thread.totalEvents), sortValue: thread => thread.totalEvents ?? "0" },
    { name: "Capacity", numeric: true, value: thread => count(thread.capacity), sortValue: thread => thread.capacity ?? "0" },
  ];

  async function capture(mode) {
    const warnings = { CLEAR: "Capture and clear retained events?",
      STOP: `Capture, stop recording, and release the event buffers?${editing ? " Unsaved policy edits will be discarded." : ""}` };
    if (warnings[mode] && !window.confirm(warnings[mode])) return;
    await action("Capturing snapshot", async () => {
      try {
        const snapshot = await api.capture(app.id, mode);
        if (mode === "STOP") setEditing(false);
        await onCapture(app, snapshot);
      }
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
    <ViewHeading kind="Live" title={displayName(app)}
      context={[`PID ${app.pid}`, app.owner, app.serverVersion && `Seismograph ${app.serverVersion}`].filter(Boolean).join(" · ")}>
      <div className="actions live-actions">
        <button className="primary" disabled={!!busy} onClick={() => capture("CONTINUE")}>Capture snapshot</button>
        <button className="quiet" disabled={!!busy} onClick={disconnect}>Disconnect</button>
      </div>
    </ViewHeading>
    {error && <p className="error notice" role="alert">Live activity is not current. {error}</p>}
    <DetailCard fields={[
      ["State", config && (recording(config) ? "Recording" : "Off")], ["Retained events", data && count(data.retainedEvents)],
      ["Lost events", data && count(data.lostEvents)], ["Recorder memory", data && amount(data.allocatedBytes)],
      ["Threads", data && count(data.threadCount)],
    ].map(([name, value]) => ({ name, value: value ?? "—" }))} />
    <div className="snapshot-toolbar"><ViewTabs id={tabId} label="Live sections" items={liveViews} selected={section} onSelect={setSection} /></div>
    <ViewPanel id={tabId} item="activity" selected={section}>
      <ActivityTracks history={history ?? []} totals={data?.classEvents} stale={!!error} />
    </ViewPanel>
    <ViewPanel id={tabId} item="threads" selected={section}>
      <DataGrid label="live threads" rows={threads} rowKey={thread => thread.id}
        columns={threadColumns} empty="No recorded threads." />
    </ViewPanel>
    <ViewPanel id={tabId} item="recording" selected={section}>
      <div className="actions recorder-actions">
        <button disabled={!!busy || !config || editing} onClick={() => setEditing(true)}>Edit policies</button>
        <button disabled={!!busy} onClick={() => capture("STOP")}>Capture and stop</button>
        <button disabled={!!busy} onClick={() => capture("CLEAR")}>Capture and clear</button>
        <button className="danger" disabled={!!busy} onClick={clear}>Clear buffers</button>
      </div>
      {editing ? <Policies initial={config} busy={busy} close={() => setEditing(false)} save={async value => {
        const saved = await action("Saving recording policies", async () => {
          await api.record(app.id, value);
          setGeneration(value => value + 1);
        });
        if (saved) setEditing(false);
      }} /> : <div className="surface table-scroll">
        <table aria-label="Recording policies">
          <thead><tr><th scope="col">Event class</th><th scope="col">Enabled</th><th scope="col">Backtraces</th><th scope="col">Sample rate</th></tr></thead>
          <tbody>{groups.map(([key, title]) => {
            const policy = config?.[key];
            return <tr key={key}><th scope="row">{title}</th><td>{policy ? policy.enabled ? "On" : "Off" : "—"}</td>
              <td>{policy ? policy.captureBacktraces ? "On" : "Off" : "—"}</td>
              <td>{policy?.samplingOneIn ? `1 in ${count(policy.samplingOneIn)}` : "—"}</td></tr>;
          })}</tbody>
        </table>
      </div>}
    </ViewPanel>
  </section>;
}

function ActivityTracks({ history, totals, stale }) {
  const start = history[0]?.time;
  const span = history.length > 1 ? history.at(-1).time - start : 0;
  const peak = Math.max(0, ...history.flatMap(point => point.rates));
  const ceiling = peak || 1;
  return <div className={`table-scroll activity${stale ? " stale" : ""}`} tabIndex={0} role="region" aria-label="Activity tracks">
    <table className="event-rates">
      <caption>Recorder activity{span > 0 ? ` · ${stale ? "Last available history · " : ""}${formatRate(peak)} events/s peak across tracks` : " · Waiting for activity samples"}</caption>
      <thead><tr><th scope="col">Event class</th><th scope="col" aria-label="Elapsed time">
        {span > 0 && <TimeRuler seconds={span / 1000} />}
      </th><th scope="col" className="track-count">Events/s</th><th scope="col" className="track-count" aria-label="Total this session">Total</th></tr></thead>
      <tbody>{groups.map(([key, title], index) => {
        const value = stale ? null : history.at(-1)?.rates[index];
        return <tr key={key}>
          <th scope="row" className="track-label"><span className={`event-series series-${index}`}>{title}</span></th>
          <td className="track-cell">{span > 0 && <TrackLine history={history} index={index} start={start} span={span} ceiling={ceiling} />}</td>
          <td className="track-count">{value == null ? "—" : formatRate(value)}</td>
          <td className="track-count">{totals?.[index] == null ? "—" : count(totals[index])}</td>
        </tr>;
      })}</tbody>
    </table>
  </div>;
}

function TrackLine({ history, index, start, span, ceiling }) {
  let penDown = false;
  const path = history.map(point => {
    const value = point.rates[index];
    if (value == null) { penDown = false; return ""; }
    const position = `${(640 * (point.time - start) / span).toFixed(1)} ${(28 - 26 * value / ceiling).toFixed(1)}`;
    const segment = penDown ? `L${position}` : `M${position}h.01`;
    penDown = true;
    return segment;
  }).join(" ");
  return <svg className="track-plot" viewBox="0 0 640 30" preserveAspectRatio="none" aria-hidden="true">
    <path className={`series-${index}`} d={path} strokeLinecap="round" />
  </svg>;
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

createRoot(document.getElementById("root")).render(<App />);
