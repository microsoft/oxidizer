<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Seismograph Cli Logo" width="96">

# Seismograph Cli

[![crate.io](https://img.shields.io/crates/v/seismograph_cli.svg)](https://crates.io/crates/seismograph_cli)
[![docs.rs](https://docs.rs/seismograph_cli/badge.svg)](https://docs.rs/seismograph_cli)
[![MSRV](https://img.shields.io/crates/msrv/seismograph_cli)](https://crates.io/crates/seismograph_cli)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml/badge.svg)](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Live monitoring, interactive snapshot viewing, and HTML reporting for `seismograph`.

The CLI renders common thread, stack, and runtime-event data directly.
Rallocator payloads use the built-in schema-specific renderer; unknown
sources remain visible in the source inventory.

Run `seismograph monitor` to capture a running application, or
`seismograph view "C:\captures\capture.seismograph"` to inspect a native
snapshot in the same interactive tabs without connecting to a process.
The offline viewer is read-only: use `Tab` or `1` through `8` to select tabs,
the usual arrow/Enter/Backspace navigation within tabs, and `q` or `Esc` to quit.
Large files load on a worker thread while the terminal remains responsive.
Loading still requires memory for the decoded events and their summaries.
Snapshot files do not record a wall-clock capture time.

Press `F1` for contextual help on the current panel or dialog, including
column meanings, units, metric scope, and keyboard and mouse controls.
Help is scrollable and leaves the underlying selection and drafts unchanged.
Press `F1` or `Esc` to close it.

Press uppercase `F` in either viewer to filter whole records by any complete
captured stack frame; lowercase `f` still only changes displayed stack frames.
Enter comma/whitespace-separated `crate:name`, `module:crate::module`, or
`function:crate::module::name` rules. Any include can match; exclusions win.
Matching uses symbol-path segments (not substring matches), ignores generic
type arguments and hashes, and attributes qualified impl methods to their
implementing type. Symbol ownership is not true execution lineage.
Missing or unresolved frames produce an Unknown decision when the available
frames cannot decide the rules; explicitly choose whether to show those records.
Enable backtrace recording before taking a new capture to select by code;
filtering cannot recover stacks omitted from an existing recording.
Runtime records can use their event stack or task spawn provenance.
Runtime task metadata uses the spawn stack; matching events in event mode can
also retain their associated task metadata. Spawn mode applies task spawn
provenance to associated events. I/O operations match the union of captured
frames from their start and completion: an include can match either side,
while a known exclusion on either side excludes the entire operation.
The whole pair is kept or removed, so filtering cannot invent unfinished I/O.
Retained allocations and hotspots use allocation-stack provenance, with the
complete pre-filter deallocation set preserving lifetimes rather than inventing leaks.
Use Tab/up/down to select fields, type at the end of rule fields, and use
Backspace/Delete to remove the last character. Left/right/space changes options.
Enter applies asynchronously; Esc cancels the draft. Empty rules show everything,
independently of the unknown-stack option. Applied rules persist across live
captures. Only one filter worker runs at a time; newer requests replace the
queued request while the current worker finishes, including after reconnect.
Filtering never rewrites the original file; source accepted/overwritten
counters, whole-process counters, and heap topology remain unfiltered, as
indicated in the filter banner.

Drag a shared panel border with the left mouse button to resize the panes.
Click a tab header (Info, Heaps, and so on) to select that tab.
Click a list row to select and activate it, as with keyboard selection and Enter.
Sizes are retained per tab for the current monitor or viewer session.
The Threads tab includes same-thread object activity, marked `(self)`.
Channel receive waits indicate an empty channel, not lock contention; they
remain visible as events but are excluded from contention totals and highlighting.
In the recording configuration, `on` records every event with backtraces;
select `custom` to disable backtraces or change sampling.
Configuration changes are read back from the application, and the live state
is refreshed without replacing an open configuration draft.
Runtime tasks without a known worker appear in an `unassigned` group.
An empty Runtime tab distinguishes absent instrumentation from absent activity:
enabling recording does not install runtime instrumentation in the application.
With active filters, an empty Runtime tab instead reports no matching runtime
activity and points back to the filter controls.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/seismograph_cli">source code</a>.
</sub>

