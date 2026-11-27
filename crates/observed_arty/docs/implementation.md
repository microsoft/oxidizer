# Observed Arty implementation

## Extension boundary

`ObservedExt` owns registration of Arty task-spawn hooks.
Callers configure the adapter through a sink or `ObservedOptions`.
The hook type and hook payload remain private, so the adapter can add internal
hooks without expanding its public surface.

The public options type stores a sink but does not implement `ThreadAware`.
Arty relocates the private `Sink` hook payload instead.
This boundary avoids a public relocation contract for future options fields that
workers do not need.

## Task observation

The task-spawn callback captures a `Transfer` on the submitting thread.
Arty carries that transfer to the destination worker and constructs an
`ObservedTask` there.

`ObservedTask` retains the transfer for the task lifetime.
At poll start, it creates a `TransferGuard` on the destination worker.
At poll end, it drops the guard to restore the earlier worker context.
The guard stays thread-bound because it protects thread-local enrichment.

## Resource boundaries

The adapter creates a restoration guard for every active poll.
This keeps the public Observed API focused on transfer and guard operations.
The adapter benchmark records task-spawn time and allocation costs.
Reusable guard storage becomes relevant when a benchmark identifies an allocation
cost that changes a supported workload decision.

Unit tests and integration tests cover public registration, nested context,
external submission, restoration, and the zero-allocation single-sink poll
scope. Benchmark setup and warm-up remain outside the measured workload.
