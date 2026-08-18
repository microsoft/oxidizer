# TODO

The forward-looking backlog of open work for the `internity` crate. It records
only what is still worth doing: completed items are deleted rather than marked
done, so the length of this file is a real measure of outstanding work. The
shipped architecture is documented in [`DESIGN.md`](./DESIGN.md).

## Contents

### Conformance
- [CON1](#con1) — Expose the essential reader operations as inherent methods

### Features
- [F1](#f1) — Add durable (`&'static str`) resolution to the frozen `Reader`

## Conformance

<a id="con1"></a>
### CON1 — Expose the essential reader operations as inherent methods

**Area:** `reader` · **Priority:** Medium · **Effort:** Medium
**Guideline:** [`M-ESSENTIAL-FN-INHERENT`](https://microsoft.github.io/rust-guidelines/guidelines/libraries/#M-ESSENTIAL-FN-INHERENT) (should) · **Confidence:** High
**Scope:** 5 methods on each of 2 public reader types — exhaustive

There are two separate reasons to give `LocalReader` and `ThreadedReader`
inherent methods: ergonomics (the sealed trait is currently the only door to
core behavior) and performance (`iter` is boxed even when the concrete type is
known). They touch the same methods, so they should land in one change rather
than two rounds of churn on the same public surface.

#### Motivation: the sealed trait is the only door

Every essential reader operation — `len`, `is_empty`, `resolve`/`try_resolve`,
and `iter` — lives only on `Reader`. Storing a `LocalReader` in a struct by
value, which is the capability this crate now advertises, still requires
`use internity::Reader` for a trait that by construction nobody downstream can
implement. The friction shows up in the crate's own code: the doctests all
import `Reader`, and the `Debug` impls have to spell it `Reader::len(self)`.

[M-ESSENTIAL-FN-INHERENT](https://microsoft.github.io/rust-guidelines/guidelines/libraries/#M-ESSENTIAL-FN-INHERENT)
asks that a concrete type's essential functionality be reachable inherently,
with the trait there to *abstract over* implementations rather than to gate
access to them. Thin inherent forwards would make the trait import opt-in.

#### Motivation: the boxing is paid where it is not needed

[`Reader::iter`] is declared as `fn iter(&self) -> Box<dyn Iterator<Item = (Sym,
&str)> + '_>`. The boxing is forced by object safety: `Lexicon::freeze` returns
`Box<dyn Reader>`, so the trait must stay dyn-compatible and cannot use
`impl Trait` in return position.

That cost is unavoidable at the erased boundary, but it is now paid even when
the caller holds a concrete reader. Since `LocalReader` and `ThreadedReader`
became nameable public types, a full scan of a frozen corpus allocates once and
dispatches every `next()` indirectly, for no benefit — the concrete iterator
type is statically known at those call sites.

Scans are not a rare path: rebuilding a string lookup over a frozen reader (the
recipe documented on `LocalLexicon::freeze`), serialization via
`se::SerializeReader`, and any corpus-wide export all go through `iter`.

#### Proposed design

Give both concrete readers inherent versions of the essential operations,
returning an unboxed iterator, and leave the trait methods as the erased
fallback:

```rust
impl LocalReader {
    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;
    pub fn resolve(&self, sym: Sym) -> &str;
    pub fn try_resolve(&self, sym: Sym) -> Option<&str>;
    pub fn iter(&self) -> impl Iterator<Item = (Sym, &str)> + '_;
}

// ...and the same set on ThreadedReader.
```

Rust resolves inherent methods before trait methods, so a call on a concrete
reader picks up the inherent version automatically, while generic `R: Reader`
code continues to use the trait. Each trait method becomes a one-line forward to
its inherent counterpart — `iter` boxing the inherent iterator — so the
traversal and lookup logic still exists once.

Returning `impl Iterator` rather than a named struct keeps `ThreadedReader`'s
nested `flat_map` chain from leaking into the public API; the trade is that
callers cannot name the iterator type. Named types can be introduced later if
that proves limiting.

#### Compatibility

This is a source-breaking change in one narrow case: code that binds the result
of a concrete `reader.iter()` directly to a `Box<dyn Iterator<...>>` stops
compiling, because the call now resolves to the inherent method. The boxed form
remains reachable as `Reader::iter(&reader)`. The trait, its dyn-compatibility,
and `Lexicon::freeze` are all unaffected.

Verify with the workspace `semver` check before landing.

[`Reader::iter`]: https://docs.rs/internity/latest/internity/trait.Reader.html#tymethod.iter

**Done when:** `LocalReader` and `ThreadedReader` each expose `len`,
`is_empty`, `resolve`, `try_resolve`, and an unboxed `iter` as inherent
methods, every `Reader` method forwards to its inherent counterpart, the
crate's own doctests and `Debug` impls no longer need the `Reader` import, and
the workspace `semver` check has been run.

## Features

<a id="f1"></a>
### F1 — Add durable (`&'static str`) resolution to the frozen `Reader`

**Area:** `reader`, `lexicon` · **Priority:** Medium · **Effort:** Medium

#### Motivation

The [`ustr`](https://github.com/anderslanglands/ustr) crate performs very well
but relies on **leaky process-global state**: its 8-byte `Ustr` handle points
into a bump allocator that is never freed, so `as_str()` yields `&'static str`
and `as_char_ptr()` a permanent null-terminated C pointer. That makes `ustr`
trivially usable for FFI and for anywhere a stable, permanent string reference
is needed.

We want internity to offer an equivalent capability — permanent `&'static str`
references suitable for FFI — **without** hiding a global singleton and without
changing the 4-byte `Sym` handle.

#### Proposed design

Add durable resolution to the **frozen `Reader`**, leaking (only) the backing
string payload on demand:

- Keep the handle as the existing 4-byte `Sym`.
- Durable references are available **only from a frozen `Reader`**. The
  lifecycle stays: lexicons intern; readers resolve, iterate, and optionally
  leak immutable payloads.
- Leaking is opt-in and pay-as-you-go:
  - `LocalReader`: its entire string payload becomes permanent.
  - `ThreadedReader`: only the referenced shard payload becomes permanent.
  - Offset tables and other reader metadata remain reclaimable.
  - Repeated calls return slices from the same payload with no extra
    allocation.

#### Minimal API (preferred variant)

Make `LocalLexicon` **not** implement `Reader`, so every `Reader` is necessarily
frozen. Then durable resolution can be added directly to the existing `Reader`
trait, and **no other API needs to change**:

```rust
pub trait Reader: Sealed + Send + Sync {
    fn try_resolve(&self, sym: Sym) -> Option<&str>;

    /// Returns a permanent reference, intentionally leaking its backing buffer.
    fn try_get_durable_str(&self, sym: Sym) -> Option<&'static str>;

    fn get_durable_str(&self, sym: Sym) -> &'static str {
        self.try_get_durable_str(sym)
            .expect("internity: Sym does not belong to this reader")
    }

    // Existing methods...
}
```

Existing freeze signatures remain unchanged:

```rust
LocalLexicon::freeze()    -> LocalReader
ThreadedLexicon::freeze() -> ThreadedReader
Lexicon::freeze()         -> Box<dyn Reader>
```

Only two public changes are needed:

1. Remove the `Reader` impl from `LocalLexicon`.
2. Extend `Reader` with durable resolution (`try_get_durable_str` /
   `get_durable_str`).

(An alternative sketch introduced a separate `FrozenReader: Reader` trait with
`try_resolve_durable` / `resolve_durable`, but folding it into `Reader` is
simpler once `LocalLexicon` no longer implements `Reader`.)

#### Miri considerations

Intentional leaks are memory-safe, but Miri's end-of-program leak checking will
still flag allocations reachable at exit. **Do not** globally disable leak
checking. Instead, durable-reference tests should retain the result in a static
root so the intentional leak is globally reachable while ordinary tests keep
detecting accidental leaks:

```rust
static DURABLE: OnceLock<&'static str> = OnceLock::new();

let value = reader.get_durable_str(sym);
DURABLE.set(value).unwrap();
drop(reader);
assert_eq!(*DURABLE.get().unwrap(), "value"); // survives reader drop
```

#### Trade-off vs `ustr` (freeze constraint)

Unlike `ustr` — whose process-global interner accepts new strings indefinitely
and whose values give stable string access immediately — internity requires a
**fill → freeze** boundary: strings are interned during a build/fill phase, the
lexicon is consumed/frozen, and only then can a `Sym` resolve normally or yield
a leaked `&'static str`. No new strings can be interned into a lexicon after
freeze.

This suits **build-then-serve / static-dictionary** workloads (startup-bounded
interning) well, and is a poorer fit for consumers that intern unbounded new
strings throughout steady-state execution. Positioning: a specialized
static-dictionary / build-then-serve alternative to `ustr`, not a drop-in
replacement for its unbounded global interner.

Possible future extensions to widen applicability without abandoning freezing:
multiple frozen epochs/readers, overlay lexicons, or batch rotation — mind the
handle/reader association hazards (`Sym`s are only valid against their own
reader).

**Done when:** `LocalLexicon` no longer implements `Reader`, `Reader` exposes
`try_get_durable_str` / `get_durable_str`, a durable reference outlives the
reader that produced it, only the referenced shard's payload is leaked on
`ThreadedReader`, repeated calls allocate nothing further, and the Miri leak
checker stays enabled with the intentional leak held in a static root.
