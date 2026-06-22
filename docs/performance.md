# Performance

This chapter covers the principles for performance optimization work in this
workspace: when to add `#[inline]`, when to deviate from idiomatic Rust, when to
file a performance issue, and a workspace-wide reminder that runtime memory
allocation is something we go out of our way to avoid.

## `#[inline]` annotations

`#[inline]` is a hint to the compiler to consider inlining a function. Throughout
this section, "generic" includes any function that needs monomorphization — a
function counts as generic if it takes type parameters of its own or if it is
defined in a generic type or `impl`, even when the function itself takes no type
parameters.

Apply the first matching rule:

1. **Apply `#[inline]` to non-generic functions exported from the crate that sit
   on a hot path** based on your knowledge of how the API is used. Act on this
   knowledge alone, accepting some documentation noise — without the annotation
   the compiler has no opportunity to inline the function into downstream
   consumers, and we want to give it that opportunity even if no current
   benchmark measurably benefits (a future workload or customer case might).

2. **Otherwise, only apply `#[inline]` if benchmarks or disassembly show a
   generic or same-crate function on a hot path is not being inlined.** These are
   already inlining candidates by default, so the annotation is an extra hint
   applied only when measurement shows the default decision is wrong. Verify with
   `just package=<pkg> bench-cg` before and after, comparing instruction counts;
   revert if the numbers do not move.

3. **Do not use `#[inline(always)]` or `#[inline(never)]` without specific
   justification.** These are stronger hints intended as advanced tuning knobs;
   general-purpose code should not reach for them.

## Performance optimization principles

When proposing or applying performance optimizations:

* **Optimizations must be motivated by user-facing scenarios, not raw benchmark
  deltas.** A Callgrind win on a synthetic micro-benchmark is not by itself
  sufficient justification. Ask: *what real workload does this help, and is that
  workload a design target of this package?* If the answer is "I would have to
  invent one", the change should usually not land.
* **Prefer surgical interventions over architectural rewrites.** A 5-line
  `#[inline]`, a single-field type change (`fn` → `Option<fn>`), or an
  `unreachable!()` → `unreachable_unchecked!()` swap is the right shape of
  optimization PR when measurement points at a specific instruction the compiler
  is emitting. Multi-file restructurings (changing in-memory representation,
  monomorphizing on type traits, deferring initialization, swapping data
  structures) are an order of magnitude harder to land and need correspondingly
  stronger motivation.
* **Preserve defensive runtime checks even if they cost a handful of
  instructions.** A runtime `unreachable!()`, `debug_assert!`, or
  `Option::expect` arm is often there to surface thread-safety bugs, state-machine
  corruption, or other "this should never happen but if it does we want to know"
  conditions. Do not remove these to save a `cmpq` — if you have a measured need
  to remove the check, prefer the surgical alternative
  (`unreachable_unchecked!`, `debug_assert!` instead of `assert!`, etc.) that
  keeps the contract documented.
* **Stay idiomatic Rust.** Manually controlling memory representation (`repr(C)`
  for layout stability, `alloc_zeroed` on hand-crafted layouts, explicit
  discriminant encoding) leans toward "coding Rust as if it were C" and is rarely
  worth the resulting reader confusion and soundness review burden. Trust the
  compiler's layout decisions unless there is a concrete cross-language interop
  or ABI requirement that forces your hand.
* **First-insert and teardown costs are usually not worth optimizing.** Most data
  structures in this workspace target long-lived, steady-state workloads.
  Optimizations whose entire value is in the construction or destruction path
  (first allocation, bulk drop, etc.) rarely pay for the complexity they
  introduce.

When filing a performance issue, state explicitly which of these criteria the
proposal meets. If you have to invent a scenario to motivate it, the issue should
probably not be filed.

Some packages have package-scoped optimization guidance that refines these
principles for their domain. Always check for a package-local
`AGENTS.md` when planning optimization work in a specific crate.

## Memory allocation is the root of all evil

Avoid algorithms that allocate memory at runtime when an allocation-free
alternative is available.

## Justify deviations from standard patterns

When you reach for a hand-rolled construct or non-standard pattern in place of
the obvious ecosystem default — for example, `hashbrown::HashTable` instead of
`std::collections::HashMap`, a custom intrusive container instead of
`Vec`/`VecDeque`, a bespoke synchronization primitive instead of `std::sync`,
manual `Pin`/`UnsafeCell` plumbing instead of safe wrappers, an internal trait
re-implementation instead of using a library trait — you must explain *why* in a
comment next to the deviation.

The justification should cover:

* What standard pattern the reader would expect to see here and is not seeing.
* Which alternatives were considered and ruled out, with the concrete reason each
  was rejected (e.g. "unstable on Rust 1.95", "fails NLL borrow-check", "trait
  bound `X: Y` does not hold for our key type", "allocates per call").
* What the chosen variant buys us that the standard pattern does not.

Without this, the next reader will reasonably assume the deviation is accidental
or unnecessary and try to "fix" it back to the standard pattern. The comment is
what prevents that wasted cycle.
