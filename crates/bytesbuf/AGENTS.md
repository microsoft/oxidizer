# AI Agents Guidelines for `bytesbuf`

Code in this crate should follow the [Microsoft Rust Guidelines](https://microsoft.github.io/rust-guidelines/agents/all.txt).

## Design Assumptions

These assumptions shape how we write and optimize code in this crate. Keep them in mind when
proposing or reviewing changes, especially performance-related ones.

### Thread-isolated architecture (mutexes are never contended)

Our types are intended to be used in a thread-isolated architecture where mutexes are almost
never contended. Any mutex that exists does so for safety in degenerate scenarios (e.g. memory
allocated on one thread but released on another), not because we expect concurrent access on the
hot path. We never optimize for contention and we never expect a mutex to be contended.

### Spans per buffer/view (0 to hundreds)

We expect our buffers and views to consist of different numbers of spans in real-world
scenarios, ranging from 0 to hundreds. Code must handle the full range correctly, not just the
small-count case.

### Inline spans are performance-critical

Assembling a buffer/view from a small handful of existing spans is a performance-critical
scenario for some media services and low-level network services. Therefore, we inline a small
number of spans directly in our buffer/view objects, avoiding extra heap allocations for the
common case. We accept that this makes our buffer/view objects larger as a deliberate trade-off.

### Metrics-driven fine-tuning (via the `nm` crate)

We rely on metrics emitted via the `nm` crate to fine-tune our constants and algorithms according
to real-world customer data. Fine-tuning for synthetic test data is not a priority. The `nm` crate
is designed for low-level metrics and is low overhead — acceptable to emit even on hot paths in
release builds — so do not remove or feature-gate these metrics purely on hot-path performance
grounds.

### Buffer/view size matches the unit of work

Our buffers and views are intended to match the unit of work size in the customer operation. For
example, the chunk size when copying data or performing I/O, or an entire HTTP request/response
when performing buffered HTTP. Design and tuning decisions should assume buffers/views are sized
to one unit of work, not arbitrarily large or arbitrarily small.
