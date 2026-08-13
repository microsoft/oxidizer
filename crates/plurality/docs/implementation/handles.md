# Handles

This document covers the four owning handle flavors: how each is represented,
what its drop path does, how its auto-trait and variance behaviour arises, and
the shared surface they are generated from. Back to the
[implementation hub](../IMPLEMENTATION.md). For the user-visible model see
[the handle design](../design/handles.md).

## The four flavors

| | `Box<T: ?Sized, A>` | `Arc<T: ?Sized, A>` | `Rc<T: ?Sized, A>` | `Alloc<'pool, T, A>` |
|---|---|---|---|---|
| Ownership | Unique, detached | Shared, atomic, detached | Shared, non-atomic, detached | Unique, bound to the pool borrow |
| Payload | `NonNull<T>` | `NonNull<T>` | `NonNull<T>` | `NonNull<SlotCell<T>>` |
| Slot counter | Untouched | Reference count | Reference count | Untouched |
| Pool refcount | One unit | One unit | One unit | None |
| `?Sized` | Yes | Yes | Yes | No |
| Pinning | Full | At construction | At construction | None |

The three detachable flavors store a pointer to the **value**, not to the
slot. That is possible because the value is field 0 of a `#[repr(C)]` slot, so
the two addresses coincide, and it is what keeps a handle to a sized value one
pointer wide: everything else the handle needs is recovered by arithmetic (see
[the pool body](./pool-body.md)).

## `Box`

The unique detached owner. Its drop path is unconditional: run the destructor,
push the slot, release one unit of the pool reference count, and tear the pool
down if that unit was the last. It reads no counter, because uniqueness is a
property of the type rather than of a count.

It is `Send` when `T: Send` and the allocator is `Send + Sync`, and `Sync` when
`T: Sync` and the allocator is `Send + Sync`. The allocator bound is the
stronger of the two directions on purpose: a `Box` that outlives its pool may
run teardown, and teardown uses the allocator through a shared reference on
whatever thread the last handle happens to die.

`Box` carries the raw round trip — into a pointer and back — and the full
pinning surface, including a pinned view of the value and the conversion from
an owned to a pinned handle. `DerefMut` is available when `T: Unpin`, which is
what keeps a pinned `Box` from handing out a mutable reference to a value whose
address was promised to be stable.

## `Arc`

The atomically shared detached owner. Cloning increments the slot counter with
`Relaxed`; dropping decrements with `Release` and, on the transition from one
to zero, issues an `Acquire` fence before destroying the value and pushing the
slot. Both `Send` and `Sync` require `T: Send + Sync` and a `Send + Sync`
allocator, exactly as for the standard library's counterpart plus the teardown
consideration above.

`Arc` exposes uniqueness queries — a mutable borrow when the count is one, and
pointer identity — and offers no `DerefMut`, because sharing is the point.

## `Rc`

The non-atomically shared detached owner. It performs the identical protocol
through the atomic's raw pointer, with no atomic operation and no fence, which
is sound because `Rc` is unconditionally `!Send` and `!Sync` and therefore
cannot participate in a cross-thread transfer of the value.

The counter word itself must remain an atomic in the slot: while the slot is
free, that word is the free-list link, and the free list is a cross-thread
protocol. `Rc` only borrows the word's non-atomic interpretation for the window
in which it owns the slot.

`Rc` denies the thread-mobility traits with a marker field of a `!Send`,
`!Sync` type rather than with a negative implementation, since negative
implementations are not available on stable. Under `loom` the counter accesses
switch to instrumented atomic operations with `Relaxed` ordering, because the
model checker cannot observe raw-pointer traffic and would otherwise report no
access at all.

## `Alloc`

The unique owner bound to the pool's borrow. It is the cheapest handle:
allocating and freeing skip the pool reference count entirely, because the
`'pool` lifetime already proves the pool outlives the handle.

```rust
pub struct Alloc<'pool, T, A: Allocator = Global> {
    slot: NonNull<SlotCell<T>>,
    _pool: PhantomData<&'pool Pool<T, A>>,
}
```

The single phantom does three jobs. It carries the borrow lifetime, so the
handle cannot outlive the pool. It mentions both type parameters, which a
struct must do for every parameter it declares. And it denies `Send` and
`Sync`: a shared reference is `Send` only when its referent is `Sync`, and
`Pool` is neither `Sync` nor — as a referent seen through a shared reference —
able to confer either auto trait. Phrasing the phantom as a borrow of the pool
rather than as a bare `PhantomData<&'pool ()>` is what makes the denial follow
from the pool's own thread affinity instead of from a separately maintained
marker. The auto-trait assertions in the test suite are what hold this in
place.

`Alloc` is invariant in `T`, and that invariance comes from the payload rather
than from the markers: `SlotCell<T>` contains an `UnsafeCell`, and `UnsafeCell`
is invariant in its parameter. Any phantom that supplies the lifetime therefore
leaves the variance alone.

Its drop path is monomorphized over `T` and reads the slot through the
compiler's layout of `SlotCell<T>` rather than through a geometry provider.
That choice is deliberate. It preserves the invariance
above, because reaching the slot through a value pointer would mean storing
`NonNull<T>` and silently widening the handle to covariant in `T`. And it keeps
one consumer of the slot layout independently derived, so a bug in the shared
geometry formulas fails a test rather than hiding behind its own consistency
(see [slot geometry](./geometry.md)).

One path serves both pool forms. A layout pool built
from `Layout::new::<T>()` has, by definition, the geometry the compiler
computes for `SlotCell<T>`, so a bound owner obtained from a multi pool reads
its slot at the offsets that pool used.

`Alloc` takes `T: Sized`, offers an unconditional `DerefMut`, and cannot be
pinned. Pinning it would be unsound because forgetting it ends the pool borrow
without keeping the backing storage alive, and the pinning contract requires
the storage to outlive the value.

## Auto traits, variance and unwind safety

Every handle is unconditionally `Unpin`: the handle's own address is irrelevant
to the value it points at, and moving a handle does not move the value.

The detachable handles are covariant in `T` — their payload is `NonNull<T>` —
and the bound owner is invariant, as described above. No handle declares an
explicit variance marker; variance follows from the fields, and the tests pin
the resulting behaviour rather than the mechanism.

Unwind safety is hand-implemented per handle rather than derived. `Alloc`
exposes no path to the allocator, so its implementations bound only on `T`.
The detachable handles may share the allocator with the pool and with other
detached handles, so their implementations also require the allocator to be
reference-unwind-safe. Dropping a handle destroys a `T` and returns a slot
through atomics that cannot be left in a broken state by an unwind.

## The shared surface

The forwarding impls that every handle owes — reference conversions, borrowing,
formatting, equality and ordering, hashing, pointer formatting, `Unpin` and the
raw-pointer accessors — are emitted by macros rather than written four times.
There are two variants: one for sized payloads, used by the bound owner, and
one admitting `?Sized`, used by the detachable handles. Each has a companion
that adds the mutable half for the unique owners. The unsized mutable variant
additionally requires `T: Unpin`, matching the `DerefMut` rule above.

Writing this surface by hand would be four opportunities to omit an impl from
one flavor; generating it makes the surface uniform by construction and makes
adding an impl a single edit.

## Coercion

Unsizing a user-defined smart pointer is not expressible on stable, so the
crate delegates the coercion to the compiler and carries the result as a token
holding a `fn(*const T) -> *const U`. Applying the token to a value pointer
yields the fat pointer with its metadata attached, and the handle is rebuilt
around it.

Three constructors cover the ground: a macro that produces the token from a
plain coercion expression, a dedicated constructor for the array-to-slice case,
and an `unsafe` escape hatch for coercions the macro cannot express. Because
the result is a native fat pointer, an unsized handle is two words and its
metadata lives where the language expects it, rather than in a handle-side
metadata field.

Coercion is address-preserving and provenance-preserving, which is what allows
the erased reclamation path to walk from the coerced pointer exactly as it
would from the original.

## Uninitialized placement

Every handle flavor has an uninitialized tier: an allocation returning a
handle over `MaybeUninit<T>`, and an `unsafe` conversion to the initialized
handle once the caller has written the value. This lets a caller construct in
place rather than constructing on the stack and moving.

The tier reuses the ordinary drop path, because `MaybeUninit<T>` has `T`'s
layout and therefore routes to the same slot geometry. The conversion
casts the payload pointer and forgets the source handle, transferring the slot
without touching either reference count.
