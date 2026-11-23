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

The Seismograph CLI shows thread activity, task execution, and memory allocations in your terminal.
For live data, [start a monitor in your application][__link0]
before running `seismograph monitor`.
To open a saved snapshot, run `seismograph view capture.seismograph`.

Run `seismograph monitor-web` to use the browser monitor. Open the URL that the command prints.
The server listens only on this computer. The URL contains a secret that grants access to local recorder controls.
Do not share that URL. Press `Ctrl+C` to stop the server.

The browser monitor finds live applications, shows recorder activity, and lets you change recording policies.
You can capture, inspect, download, and open native `.seismograph` files.
The proxy saves live captures in the directory where you run the command.
The server keeps the four most recent snapshots open. Live snapshots appear under their applications.
Files that you open appear under Opened files in the left pane.
Select a tab below the snapshot title to inspect its data.
Select an application in the left pane to return to live monitoring.
In Activity, event classes share a time scale. Find Capture and stop and Capture and clear under Recording.
The area tabs and application list stay available as you scroll.
Use the arrow keys to move between rows in a grid. Press Enter to open details.
If you capture the same application again, the active tab keeps your view and stack filter.
Select a row to open its details without replacing the earlier views.
In Primitives, each set of details opens below its parent grid.
Select a Runtime worker to see its tasks.
By default, Task Details opens below Workers and Tasks. Select Timing or Metadata for charts and task information.
Drag a pane tab to dock it next to another pane or group tabs together.
Drag a divider to resize panes, including the application list. The browser saves the layout.
Select Reset layout to restore the default pane positions.
Use a fixed `--port` to keep the same layout after a server restart.
Use a column heading to sort. Use the grid header to filter the current page.
Metrics in the task, primitive operation, and allocation hot spot grids sort across all pages.
The web monitor supports snapshots up to 64 MiB. Use the terminal monitor for larger captures.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/seismograph_cli">source code</a>.
</sub>

 [__link0]: https://github.com/microsoft/oxidizer/blob/main/crates/seismograph/README.md#live-monitoring
