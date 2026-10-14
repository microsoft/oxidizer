# Windows performance-counter implementation

## Preparation and identity

`MetricBindings` prepares per-event targets once. Each target selects an atomic
total and an aggregation operation. The processor's private schema input retains
the event-slot index.

[`schema.rs`](../src/schema.rs) validates names and limits, derives deployment
identities from a fixed namespace and length-delimited components, and sorts
counter definitions by native ID. Each sorted definition retains its source
index so native ordering does not change emission routing. The same resolved
schema produces both the native registration template and deployment XML.
Changing its identity-format inputs changes installed identities.

## Ownership

The processor owns its bindings, atomic totals, first retained error, and native
provider. [`native.rs`](../src/native.rs) owns the provider handle and instance.
Reference cells remain at stable addresses through native cleanup. The
image-local registry holds a strong reference while Perflib might retain their
addresses; unsuccessful shutdown retains that storage rather than freeing a
potentially live reference.

Perflib callbacks have no provider-specific context. The registry and collection
state therefore belong to one loaded Rust image, not to a process-wide Rust ABI.
Do not unload an image with a live provider. The injected native API documents
resource ownership and cross-thread obligations independently of the
publication synchronization requirement.

## Collection coordination

Emission only updates atomic totals. The first admitted START marks publication
in progress and attempts to refresh reference cells without waiting for the
registry lock. Overlapping admitted collectors use the existing snapshot.
END releases admission without acquiring the registry lock. Invalid state
retains a fault, freezes further snapshots, and surfaces through flush.

The image-wide cell cap is a conservative resource policy, not a hard real-time
guarantee. In particular, the writer marker is internal bookkeeping: Windows
does not consult it when a callback times out. The
[unresolved publication boundary](design.md#publication-safety-boundary) must not
be inferred from the ordinary START/END or Miri results.

## Validation boundaries

Injected lifecycle and processor tests exercise failure retention, checked
arithmetic, teardown ownership, and cooperation between admitted collectors.
Native tests exercise operating-system registration and error propagation.
The installed-schema consumer scenario is opt-in: it additionally needs
generated definitions and resources installed on a Windows host. Compiling
those resources does not establish discovery or correct consumer readings.

The [consumer manifest example](../examples/consumer_manifest.rs) generates the
installer input. Its consumer test carries the outstanding installed-host
validation task. Neither the test nor the library installs machine metadata.
