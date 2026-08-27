# Thread Aware Core — Design

## What this crate is

`thread_aware_core` is the **stable vocabulary** for values that adapt when a
runtime moves them between threads. It exists to be depended on permanently:
crates are meant to name `ThreadAware` and `Thread` in their own public
signatures, and this crate's job is to make sure that never becomes a liability.

It exposes one trait, one identifier and its two component ids, and nothing
else:

- `ThreadAware` — the callback a value implements to be told it has moved.
- `Thread` — where a value runs, built from `Owner`, a `std::thread::ThreadId`,
  and `NumaNode`.

Everything that makes relocation *convenient* — registries, containers,
callbacks, derive macros, runtime integration — lives in the pre-1.0
`thread_aware` crate. This crate carries only what two unrelated libraries must
agree on in order to interoperate. Implementations of `ThreadAware` for
`core`, `alloc` and `std` types ship here because they are part of that shared
vocabulary; implementations for third-party types deliberately do not.

## Why a separate crate

A vocabulary trait is only useful if everyone names the same one. If a
connection pool and a codec each define their own `ThreadAware`, a runtime that
relocates a value containing both has two unrelated traits to satisfy and no way
to treat them uniformly. The trait must come from a single crate they share.

That makes this crate's compatibility a shared constraint rather than a private
matter. Two libraries interoperate only when their public APIs resolve to the
same `thread_aware_core` instance, so an incompatible release does not
inconvenience one crate — it splits the ecosystem until every participant has
upgraded. A crate in that position has to be *boring*: small, dependency-free,
and stable far longer than the code around it. The MSRV is part of the same
bargain, and is raised deliberately rather than incidentally.

Splitting it out is what buys that. The volatile parts of `thread_aware` keep
iterating pre-1.0 while the few items that appear in public signatures stabilise
ahead of them.

## Scope

`Owner` describes a runtime; `NumaNode` describes hardware. Neither is
validated, and their guarantees differ in ways implementations depend on. A
thread id is meaningful only while that thread lives, an `Owner` only while its
runtime does, and a `NumaNode` is shared by every thread near the same memory —
including threads of a different runtime, but only while all of them number the
nodes identically.

`Owner` values are issued by runtime and integration code, never by ordinary
data-structure authors. Every new owner is unique, so two runtimes alive at once
cannot collide however carelessly they are set up. An owner also reports the
smallest number of threads its runtime runs: a floor to pre-size against, which
the runtime may exceed, and which is zero for one that spawns on demand.

Implementing `ThreadAware` also commits a type to `Send`, since that is a
supertrait. A type holding an `Rc` or another thread-bound handle cannot
participate at all, even with an empty `relocate`.

## Principles

**Performance, not correctness.** `relocate` is advisory. A value must be fully
correct if it is never called, called twice, or called with a `Thread` it has
never seen. This is the most important property in the design: it is what lets a
runtime call `relocate` opportunistically, lets an implementation give up and
stay slow rather than fail, and lets the method be infallible. A trait whose
correctness depended on being called would need error reporting, ordering
guarantees, and a way to refuse a move — none of which this crate has.

**Say only what interoperability requires.** Every item here is a permanent
commitment, so the bar for adding one is whether two independent crates must
agree on it. A helper that merely makes implementation pleasant does not
qualify; it belongs in `thread_aware`, where it can still change.

**Borrow from `std` rather than invent.** The thread coordinate is
`std::thread::ThreadId`, not an id of our own: a bespoke type would be one more
thing to convert to and from, and one more thing to keep stable forever. The
cost is that `Thread::new` and `Thread::id` need the `std` feature. With
default features off, a `no_std` crate can still implement `ThreadAware` and
read `Owner` and `NumaNode` from the `Thread` values it is handed; it just cannot
construct one, which only runtimes need to do. Such a build needs `alloc` and
pointer-width atomics, the latter because `Owner` identities come from a
process-wide counter.

**No dependencies reach a consumer.** The manifest's only entry is a test-only
dev-dependency, so adopting this crate cannot introduce a version conflict or
pull anything unexpected into a build.

**Describe, do not enforce.** Nothing checks that runtimes number NUMA nodes
alike or that implementations avoid blocking. These are documented
obligations; enforcing them would need runtime state and a coordination point,
and this crate has neither.

## Evolving without breaking changes

The crate has to be able to describe hardware it does not model yet. Machines
already subdivide a NUMA node into cache domains, and a `Thread` may one day need
to describe a value pinned to memory but not to a processor. Both mean
adding a coordinate, and the design makes that additive.

**Ids are opaque, and take no `From<integer>`.** `NumaNode` exposes no accessor
returning its integer, and both ids are built through inherent constructors
rather than a `From` impl. That is a deliberate refusal, because a
`From<integer>` on a permanent vocabulary type is a one-shot commitment. While
one such impl exists it silently governs the width every unsuffixed literal
infers; adding a second either breaks those call sites — `from(1)` falls back to
`i32` and stops compiling — or, when the added impl happens to be `From<i32>`,
*silently re-resolves them to the new impl* with no error at all. Replacing the
impl instead breaks every caller passing a typed variable. None of these are
reported by `cargo semver-checks`, so adding one would reach a release
unchallenged.

An inherent constructor avoids the trap entirely: `u32` already exceeds any node
count real hardware reaches, and if a wider or fallible form is ever needed it
arrives as a second, differently named constructor, which is purely additive.
The *stored* width remains private and may grow silently — it is not part of the
promise, and growing it is what would reserve values no existing constructor can
reach.

**`Thread` fields are private.** Coordinates are read through accessors, so the
struct can gain a field without touching any existing reader, and the derived
`Clone`, `Debug`, `PartialEq`, `Eq` and `Hash` pick it up automatically. No
`#[non_exhaustive]` is needed: with no public fields there is nothing to
destructure or update.

What a new field *can* silently remove is an auto trait — a coordinate that is
not `Send`, `Sync` or unwind-safe strips that property from `Thread` without any
signature changing. Static assertions pin the auto traits the public types are
expected to have, so this fails the build rather than reaching a release.

**Every coordinate has a default variant.** This is the rule that makes the rest
work: a new coordinate must have a value meaning *unknown, or not pinned* —
something like a `NumaNode::UNPINNED` constant that no ordinary construction can
produce. Existing constructors keep their exact signatures and fill the new
coordinate with that default; callers that care about it opt in through a new
constructor.

Reserving such a value costs one representation state that no constructor hands
out. Since `new` accepts the full `u32` range, a sentinel would come either from
widening the private storage past it or from making the reserving constructor
fallible, so that the reserved value can never be minted. A coordinate type
introduced later can simply reserve one from the start.

Behaviour is preserved as well as compilation. Every pre-existing construction
path fills the new coordinate identically, so values built the old way remain
equal to exactly the values they were equal to before; the new coordinate can
only tell them apart once someone deliberately sets it.

Adding a coordinate is therefore three additive steps:

1. Add the private field and its accessor.
2. Add the sentinel constant to the coordinate's type.
3. Add a constructor that accepts it, leaving the existing one alone.

**Trait additions stay defaulted and dyn-compatible.** `dyn ThreadAware` is part
of the stable surface, so a default body alone is not enough. Any method added
to `ThreadAware` must also avoid generic parameters, `impl Trait`, and `Self` by
value, or carry `where Self: Sized` so the vtable ignores it. Otherwise every
existing `impl` still compiles while every `Box<dyn ThreadAware>` stops. An
assertion pins dyn-compatibility for the same reason the auto traits are pinned.

### What this does not permit

The escape hatch is one-directional. Removing a coordinate, changing the
signature of an existing constructor, adding a `From<integer>` impl, changing
what an existing id *means*, or adding a method that breaks dyn-compatibility
are all breaking, and no sentinel value makes them otherwise. Reserving the
additive path for genuinely new information is what keeps it available.

## Related documents

For the exact list of items inside the stable boundary, see
[STABILIZATION.md](./STABILIZATION.md).
