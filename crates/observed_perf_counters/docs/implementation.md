# Windows performance-counter implementation

## Preparation and identity

`MetricBindings` prepares per-event targets once. Each target selects an atomic
total and an aggregation operation. Static field metadata and the lazy visitor
share stable field indices; omitted optional fields cannot shift later targets.
The processor's private schema input retains the prepared target's storage index.

[`schema.rs`](../src/schema.rs) parses the externally authored V2 manifest,
validates its supported native profile, and selects one set by category name.
The supplied GUIDs and counter IDs are retained verbatim. Binding matches names
once, resolves event fields to storage indices, and rejects unknown or conflicting
destinations. Unbound declared counters receive zero-valued storage.

Counter definitions are sorted by their declared native IDs. Each definition
retains its resolved storage index, so native order does not change event-field
routing. Native registration uses that prepared schema; there is no XML export
or application-derived identity calculation.

## Ownership

The processor owns its bindings, atomic totals, first retained error, and native
provider. [`native.rs`](../src/native.rs) owns the provider handle and instance.
The references registered with Perflib point directly at the processor's atomic
totals, not at a second publication buffer. The image-local registry holds a
strong reference while Perflib might retain their addresses; unsuccessful
shutdown retains that storage rather than freeing a potentially live reference.
Each registration has its own registry identity, even if totals are shared.

Do not unload an image with a live provider. The registry belongs to one loaded
Rust image, not to a process-wide Rust ABI. The injected native API documents
resource ownership, full-width atomic reads, and cross-thread obligations.

## Continuous publication

Emission updates the registered atomic totals directly. Registration and
teardown use the registry lock; collection does not acquire it. The custom
callback returns success without accessing state or native input buffers.
No copy, reader count, callback ordering, deadline, or quiescence assumption
participates in publication.

The [native access contract](design.md#publication-safety-boundary) restricts
this design to aligned, full-width eight-byte counter reads on 64-bit Windows.
Individual readings are atomic; several readings need not represent one event
or one point in time. The image-wide counter cap is a resource policy, not a
real-time or synchronization guarantee.

## Validation boundaries

Injected lifecycle and processor tests exercise failure retention, checked
arithmetic, teardown ownership, registered-pointer identity and alignment, and
concurrent full-width atomic reads. A deterministic delayed-START test lets a
consumer read updated totals and issue END before the START callback resumes.
There are no sleeps and no deliberately conflicting non-atomic accesses.
Miri validates Rust ownership, provenance and synchronization in that model;
it does not run Perflib or establish its native access contract.
Native tests exercise operating-system registration and error propagation.
The installed-schema consumer scenario is opt-in: it additionally needs
the supplied definitions and resources installed on a Windows host. Compiling
those resources does not establish discovery or correct consumer readings.

The [consumer definition fixture](../tests/test_data/ConsumerCounters.man) is the
installer input for the opt-in consumer test. Its GUIDs and IDs are fixed
independently of the implementation. The test carries the outstanding installed-
host validation task. Neither the test nor the library installs machine metadata.
