# internity TODO

## Durable (`&'static str`) resolution — "static string leak" mode

### Motivation

The [`ustr`](https://github.com/anderslanglands/ustr) crate performs very well
but relies on **leaky process-global state**: its 8-byte `Ustr` handle points
into a bump allocator that is never freed, so `as_str()` yields `&'static str`
and `as_char_ptr()` a permanent null-terminated C pointer. That makes `ustr`
trivially usable for FFI and for anywhere a stable, permanent string reference
is needed.

We want internity to offer an equivalent capability — permanent `&'static str`
references suitable for FFI — **without** hiding a global singleton and without
changing the 4-byte `Sym` handle.

### Proposed design

Add durable resolution to the **frozen `Reader`**, leaking (only) the backing
string payload on demand:

- Keep the handle as the existing 4-byte `Sym`.
- Durable references are available **only from a frozen `Reader`**. The
  lifecycle stays: lexicons intern; readers resolve, iterate, and optionally
  leak immutable payloads.
- Leaking is opt-in and pay-as-you-go:
  - Flat reader: its entire string payload becomes permanent.
  - Sharded reader: only the referenced shard payload becomes permanent.
  - Offset tables and other reader metadata remain reclaimable.
  - Repeated calls return slices from the same payload with no extra
    allocation.

### Minimal API (preferred variant)

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
LocalLexicon::freeze()   -> impl Reader
ThreadedLexicon::freeze() -> impl Reader
Lexicon::freeze()        -> Box<dyn Reader>
```

Only two public changes are needed:

1. Remove the `Reader` impl from `LocalLexicon`.
2. Extend `Reader` with durable resolution (`try_get_durable_str` /
   `get_durable_str`).

(An alternative sketch introduced a separate `FrozenReader: Reader` trait with
`try_resolve_durable` / `resolve_durable`, but folding it into `Reader` is
simpler once `LocalLexicon` no longer implements `Reader`.)

### Miri considerations

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

### Trade-off vs `ustr` (freeze constraint)

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
