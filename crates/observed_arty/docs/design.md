# Observed Arty design

## Purpose

`observed_arty` transfers Observed enrichment into async tasks that Arty runs.
Enrichment contains context fields that belong to events, such as a request ID.

The integration captures enrichment from the thread that submits a covered task.
Each task poll receives the captured fields before it runs.
The integration restores the worker's earlier enrichment after the poll.

## Public behavior

Call `ObservedExt::enable_observed` on a runtime builder or a live runtime.
The caller can pass an owned `Sink`, a shared `&Sink`, or `ObservedOptions`.
The public extension keeps hook registration inside the adapter.

Builder registration covers async tasks submitted after runtime construction.
Live registration takes effect on the calling worker immediately.
Other workers apply it when they process the registration.

The integration does not cover blocking functions submitted through `spawn_blocking`.
The adapter transfers the complete capture state, so later task-local enrichment
follows the captured entries when code visits the enrichment list.
