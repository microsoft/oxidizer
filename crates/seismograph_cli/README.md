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

**Native v4**, immediately left of Allocations, explores the global backend
and every inventoried owner endpoint, including active never-observed owners.
The top diagram follows OS reservations through the shared backend to owners,
slabs, ranges, caches and returns. Select Memory, Global backend or an owner
with Up/Down, then Enter to focus category details, subsystem choices and class details.
Escape/Backspace returns one level without leaving the capture; Escape exits
only at the root. Home/End and PgUp/PgDn navigate long lists or focused details.
Mouse rows focus their list, and cyan borders identify keyboard focus.
Contextual F1 help contains metric meanings and coverage limitations; regular
panels show only structures, concise metrics and observation badges.
OS reserved combines cumulative native OS reservations (including metadata
backing) and separate sparse page-map virtual address space using 128-bit
arithmetic. Committed, resident and swapped memory are explicitly Unknown.
Observed, Older, Unknown, Busy, Unavailable and Partial badges preserve missing
evidence without inventing zero measurements or current-lease attribution.
Native capacity is not application-live memory, batching budget is not pending
bytes, and global cached ranges are not guaranteed physically decommitted.
Self-publication defaults on but costs nothing with recording off. Capture
requests the next publication round only after collection. Operations publish
only after an accepted recorded allocation/free event and the native operation
completes; sampled-out events and merely enabled attempts do not contribute.
Explicit app-side requests also work, without timers or background workers.
Offline files never request new publications.

In **Allocations**, press `e` to switch between grouped hotspots and individual
allocation/free records, including actor names, operation stacks, requested
sizes, addresses, view-local lifetime IDs and explicit orphan frees.
Address reuse does not merge retained lifetimes; filtering preserves original
correlation IDs and whether a free had a retained allocation origin.
Export the same native inventory with
`seismograph snapshot html capture.seismograph report.html`.
Schema 3 is required; older allocator payload schemas are rejected.

Run `seismograph monitor` to capture a running application, or
`seismograph view "C:\captures\capture.seismograph"` to inspect a native
snapshot in the same interactive tabs without connecting to a process.
The offline viewer is read-only: use `Tab` or `1` through `8` to select tabs,
the usual arrow/Enter/Backspace navigation within tabs, and `q` or `Esc` to quit.
Large files load on a worker thread while the terminal remains responsive.
Loading still requires memory for the decoded events and their summaries.
Snapshot files do not record a wall-clock capture time.
The Info view identifies the local `seismograph_cli` crate version and the
connected server’s `seismograph` crate version, even before capture.
Legacy servers report an unknown version; offline snapshots do not store
their producer’s version but still show the local monitor version in Info.
Live Info charts accepted-event rates in separate colors for allocations,
general events, Arc dereferences, runtime tasks, I/O, and cache events.
The lower-right thread list shows each recorder on one line: identity, actual
ring fill, current event rate and a recent-rate sparkline. Use Up/Down, PgUp/PgDn,
Home/End, the mouse wheel, or click a thread to browse every recording thread.
These lightweight counters refresh without a snapshot or stack symbolization;
snapshot filters do not affect them. New recording sessions reset rate baselines.
Threads that have never recorded an event are not enumerated OS threads.
Older servers retain the total-rate chart and explicitly mark detailed counters
unavailable; rebuild the application as well as the monitor for the new view.
Metric caveats are in F1 help rather than below the Info metrics.
Runtime task rows show completed worker-local poll counts, observed execution
fraction, median poll duration, maximum poll duration and inline future bytes.
Future size excludes executor bookkeeping and separately allocated buffers.
A dash indicates unavailable metadata or observations, not a measured zero.

In live mode, `s` captures using one of two actions selected with `d`:
**Record and continue** (default) preserves server buffers and recording
policy, so enabled classes continue writing and disabled classes stay off.
**Record and stop** captures events, disables all six recording classes,
and deallocates their ring buffers. Process-lifetime recorder metadata remains.
Exited-thread buffers are released after either capture action.
Uppercase `C` independently clears server buffers without a snapshot,
symbolization or file I/O, keeping active-thread allocations and recording
policy. Clear never removes the displayed capture or saved files.
Lowercase `c` still configures each recorder’s off/on/custom policy.
Capture, configuration and Clear are serialized by the UI. Stop and Clear
require server support and never fall back to legacy release behavior.
A source, decoding or save failure after Stop cannot restore discarded rings;
the monitor reads back live recording policy even when the capture fails.

Press `F1` for help scoped to the focused subpanel or dialog, including
column meanings, units, metric scope, and keyboard and mouse controls.
Non-focusable child charts and stacks are included; sibling panels are not.
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
counters and native owner/backend inventory remain unfiltered, as
indicated in the filter banner.

Drag a shared panel border with the left mouse button to resize the panes.
Click a tab header (Info, Native v4, and so on) to select that tab.
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

Runtime shows worker poll timelines above a task table, compact worker activity,
and adjacent **Statistics**. Press `t` or click **Poll / Ready** to switch the
task histogram; F1 explains its worker-local and task-global metrics.
Left/Right (or Up/Down) and PgUp/PgDn select one/five histogram buckets.
The bottom row keeps **Operations**, **Task occurrences**, and **Event stack**
side by side; `e` focuses it without hiding the dashboard.
Enter drills through `Workers -> Tasks -> Statistics -> Operations -> Occurrences`;
Backspace reverses that path. At narrow sizes only the focused panel is shown.
Events infers the task’s operations across workers from retained poll boundaries,
without changing event recording. Occurrences show only that task’s selected
operation, ordered by time with original thread, sequence and object identity.
Other actors and other operation kinds never enter this list, even on shared
objects. Unknown or ambiguous actors stay unassigned.
Allocation origin and freeing actor are distinct, not task ownership or live memory.
Task roots come from poll stacks or recognized instrumented future wrappers
in event stacks; without either, stacks remain explicitly untrimmed.
Lowercase `f` switches to the full captured stack, including executor frames.
In the event browser, PgUp/PgDn scrolls the stack and Left/Right pans long frames.
Tab and Shift-Tab always cycle main tabs; there is no text-details toggle.
`[`/`]` change the task sort and `r` reverses it.

Observed execution is the union of poll intervals clipped to the displayed
per-runtime window, divided by that window’s duration: a lower bound, not CPU
utilization. Timelines use eight rising bar heights with a visible baseline;
dim baseline bars indicate unobserved data, not measured idle time.
Every positive observed fraction rises above the zero baseline.
Duration histograms show 12 fixed log10 buckets horizontally and sample counts vertically:
`0..<10ns`, `10..<100ns`, and successive decades through milliseconds and seconds, ending in `100s+`.
Extremely narrow panels merge adjacent buckets.
Bars are one character wide, with a gray `_` baseline for every bucket.
Nonempty buckets rise at least a one-eighth-cell sliver above it; numeric counts remain exact.
The timeline shows at most the latest 60 seconds of event/source observations;
filters preserve this common original axis. Counts and histograms still use all
retained completed samples, and source ages are shown in full.
Poll counts and duration statistics use retained completed polls on the selected
worker. Ready wait is task-global queue time, not poll duration or raw wake
latency; a wake during a poll starts ready wait only when that poll finishes.
If coherent completed queue samples are absent, the right histogram instead
shows available raw wake-to-poll latency, explicitly labeled as including
self-wake overlap with Running. Raw and queue samples are never mixed.
Missing duration samples show `-`, while a measured zero shows `0ns`.
Histogram axes show inclusive duration ranges in logarithmic buckets, with
rounded unit-labelled limits and linear sample counts. Open source intervals
appear in timelines, not histograms.
Running/Ready ages come from coherent per-task source observations, not a
globally atomic snapshot; recording stop freezes those ages. Older captures
and raced observations have unknown activity. F1 explains these boundaries,
including incomplete, overwritten and filtered evidence, from every Runtime panel.
Open polls use their coherent poll-worker identity, not separately sampled
last-worker or worker-slot metadata. Older activity schemas without that
identity require an exact retained task/runtime/poll-start timestamp match to
assign worker occupancy; otherwise only the global running age is shown.
Waiting records a poll exit without an outstanding wake, not proof of
`Poll::Pending`. Waiting or Ready can briefly appear between the poll-exit
hook and terminal retirement after completion or panic.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/seismograph_cli">source code</a>.
</sub>

