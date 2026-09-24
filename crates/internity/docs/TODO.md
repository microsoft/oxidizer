# TODO

The forward-looking backlog of open work for the `internity` crate. It records
only what is still worth doing: completed items are deleted rather than marked
done, so the length of this file is a real measure of outstanding work. The
shipped architecture is documented in [`DESIGN.md`](./DESIGN.md).

## Contents

### Conformance
- [CON1](#con1) — Expose the essential reader operations as inherent methods

### Features
- [F1](#f1) — Add durable (`&'static str`) resolution to frozen readers

## Conformance

<a id="con1"></a>
### CON1 — Expose the essential reader operations as inherent methods

**Area:** `reader` · **Priority:** Medium · **Effort:** Medium
**Guideline:** [`M-ESSENTIAL-FN-INHERENT`](https://microsoft.github.io/rust-guidelines/guidelines/libraries/#M-ESSENTIAL-FN-INHERENT) (should) · **Confidence:** High
**Scope:** 5 methods on each of 2 public reader types — exhaustive

`LocalReader` and `ThreadedReader` expose `len`, `is_empty`, `resolve`,
`try_resolve`, and `iter` only through the sealed `Reader` trait. Callers
holding a concrete reader must import that trait; its object-safe `iter`
also boxes an iterator even when the concrete iterator type is known.

- `crates/internity/src/local_reader.rs:105` — the reader implements `Reader`
  without inherent counterparts.
- `crates/internity/src/threaded_reader.rs:66` — same for the sharded reader.

Expose these operations inherently on each frozen reader, with an unboxed
`iter` for concrete callers; retain the object-safe trait methods as the
erased fallback. Inherent methods take precedence over trait methods, so
callers that explicitly expect a boxed iterator may need to use
`Reader::iter(&reader)`. Verify that source compatibility trade-off before
landing the change.

**Done when:** both concrete readers expose all five methods inherently,
their `Reader` methods forward to the same lookup and traversal logic, and
the semver check documents any source-compatibility impact.

## Features

<a id="f1"></a>
### F1 — Add durable (`&'static str`) resolution to frozen readers

**Area:** `reader`, `lexicon` · **Priority:** Medium · **Effort:** Medium

Unlike a process-global interner that leaks strings for the entire process,
internity normally reclaims its buffers when frozen readers are dropped.
FFI and other consumers that need a permanent string reference currently
cannot opt into durable resolution. The original backlog proposal would
have removed `LocalLexicon: Reader` to put durable methods on `Reader`,
but that changes the existing public API and must not be done here.

Add opt-in durable resolution on the concrete frozen `LocalReader` and
`ThreadedReader` instead, without removing `LocalLexicon: Reader` or
changing the object-safe `Reader` trait. The local reader can leak its
string payload on demand; the threaded reader should leak only the referenced
shard's payload. Offset tables and other reader metadata remain reclaimable,
and repeated calls should reuse the same leaked payload. Document that a
`Box<dyn Reader>` cannot expose this frozen-only capability without an
additional API decision.

Miri's leak checker should remain enabled. Tests can retain an intentional
durable reference in a static root while continuing to detect accidental
leaks, and should check that a reference survives dropping its reader.

**Done when:** both concrete frozen readers can resolve a handle to
`&'static str` on demand without changing the existing `Reader` API, only
the referenced threaded shard's payload is leaked, repeated calls do not
leak again, and Miri distinguishes intentional from accidental leaks.
