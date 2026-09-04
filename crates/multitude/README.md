<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Multitude Logo" width="96">

# Multitude

[![crate.io](https://img.shields.io/crates/v/multitude.svg)](https://crates.io/crates/multitude)
[![docs.rs](https://docs.rs/multitude/badge.svg)](https://docs.rs/multitude)
[![MSRV](https://img.shields.io/crates/msrv/multitude)](https://crates.io/crates/multitude)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Fast and flexible arena-based bump allocator.

`multitude` allocates phase-oriented values from chunks and reclaims their
storage in bulk. Escape-capable handles can retain and reclaim individual
chunks independently.

## Key properties

1. **Flexibility.** Four allocation styles coexist in the same arena: the
   arena-lifetime owning handle [`Alloc<T>`][__link0] plus three escape-capable
   smart pointers — the atomic [`Arc`][__link1], the non-atomic single-thread [`Rc`][__link2],
   and the unique-owner [`Box`][__link3] — each available for sized `T`, `str`, and
   `[T]`. See the [comparison table](#flexibility) for how they differ.

1. **Early Reclamation.** A chunk containing only escape-capable allocations
   is reclaimed when its last handle drops.

1. **Escaping Smart Pointers.** `Arc`, `Rc`, and `Box` can outlive the arena
   by retaining their chunk.

1. **Drop Support.** Owning handles run value destructors eagerly.

1. **Compact Smart Pointers.** `multitude`’s escape-capable smart pointers —
   [`Arc<T>`][__link4], [`Rc<T>`][__link5], and [`Box<T>`][__link6] — are **8 bytes** on
   64-bit for sized values, `str`, and `[T]`. Slice lengths live in a chunk
   prefix; vtable-metadata pointees, including trait objects and custom DSTs
   with trait-object tails, additionally carry one word. The arena-lifetime
   [`Alloc<T>`][__link7] handle is a single word for sized `T`; for `str` /
   `[T]` it is a fat reference (pointer + length), which costs nothing extra
   since it never escapes the arena and isn’t stored at scale.

1. **Growable Collections.** [`String`][__link8],
   [`Utf16String`][__link9], and [`Vec`][__link10] grow in arena
   storage.

1. **Dynamically-Sized Types.** Slices, strings, trait objects, and custom
   DSTs use the same ownership forms as sized values.

1. **Serde Support.** Values can be deserialized directly into arena-backed
   storage.

1. **Formatting.** [`format!`][__link11] writes formatted strings
   directly into arena storage.

1. **UTF-16 Support.** The `utf16` feature provides arena-backed UTF-16
   strings and formatting.

1. **`#![no_std]` Support.** The core allocator requires only `alloc`.

See [`BUMPALO.md`][__link12]
for a feature-by-feature comparison with [`bumpalo`][__link13].

## Performance

See [`PERF.md`][__link14]
for measured wall-clock timings of customer-facing scenarios, including
head-to-head comparisons against the system allocator, `bumpalo`, and
standard `serde_json` deserialization.

## Example

```rust
use multitude::Arena;

let arena = Arena::new();

// Cheap atomic reference-counted allocation of any user type.
struct Point { x: f64, y: f64 }
let p = arena.alloc_arc(Point { x: 3.0, y: 4.0 });
let p2 = p.clone();
assert_eq!(p.x, p2.x);

// Single-pointer immutable strings.
let name = arena.alloc_str_arc("Alice");
assert_eq!(&*name, "Alice");

// format! macro returning a String.
let greeting = multitude::strings::format!(in &arena, "Hello, {}!", "world");
assert_eq!(&*greeting, "Hello, world!");
```

## Flexibility

`multitude` offers four ways to allocate a value and own it over time. All
four can coexist in the same arena, dereference to the value, and run
`T::drop` **eagerly**; they differ in whether the handle can outlive the
arena, whether ownership is unique or shared, and whether the pointee has a
shared strong count. Each is available for sized `T`, `str`, and `[T]`;
behind the `dst` feature, the three smart-pointer forms also support
arbitrary DSTs.

||[`Alloc<T>`][__link15]|[`Box<T>`][__link16]|[`Rc<T>`][__link17]|[`Arc<T>`][__link18]|
|-|:--------:|:------:|:-----:|:------:|
|**Constructor family**|[`alloc`][__link19]|[`alloc_box`][__link20]|[`alloc_rc`][__link21]|[`alloc_arc`][__link22]|
|**Ownership**|unique|unique|shared (`Clone`)|shared (`Clone`)|
|**`&mut T` access**|✅|✅|when uniquely owned|when uniquely owned|
|**Can outlive the arena**|❌|✅|✅|✅|
|**Shared strong count**|none|none|non-atomic|atomic|
|**Cross-thread *sharing***|❌|❌|❌ (`!Send`)|✅ (`T: Send + Sync`)|
|**Width (64-bit)**|1 word (sized); fat ref for DSTs|8 bytes; 16 for vtable metadata|8 bytes; 16 for vtable metadata|8 bytes; 16 for vtable metadata|

The cheapest option is [`Alloc<T>`][__link23]: an owning handle whose lifetime
is tied to the arena — a single word for sized `T` (a fat pointer+length
reference for `str` / `[T]`). It pays no reference count and cannot
outlive the arena, but gives mutable access and runs the destructor when it
is dropped, making it the lowest-overhead ownership form when the lifetime
constraint is tolerable.

```rust
let arena = multitude::Arena::new();
let mut x = arena.alloc(42);
*x += 1;
assert_eq!(*x, 43);

// Strings and slices too:
let s = arena.alloc_str("hello");
let v = arena.alloc_slice_copy(&[1, 2, 3]);
assert_eq!(&*s, "hello");
assert_eq!(&*v, &[1, 2, 3]);
```

For values that must **outlive the arena**, use one of the three smart
pointers. They behave like the like-named `std` types while keeping common
sized, string, and slice handles to **8 bytes**. Trait-object handles carry
an additional vtable word. All three keep their hosting chunk alive until
the last handle drops.

[`Arc`][__link24] is reference-counted and shareable across threads:

```rust
use multitude::Arc;

let p: Arc<u32> = {
    let arena = multitude::Arena::new();
    arena.alloc_arc(42)
    // arena dropped here; `p` keeps its chunk alive
};
assert_eq!(*p, 42);

let arena = multitude::Arena::new();
let shared = arena.alloc_arc(7_u64);
let h = std::thread::spawn(move || *shared);
assert_eq!(7, h.join().unwrap());
```

[`Rc`][__link25] is the single-thread sibling of [`Arc`][__link26]: its reference count is
non-atomic, avoiding atomic operations and, for low-alignment values such as
`str` / `[u8]`, the atomic count’s alignment floor. Being
[`!Send`][__link27]/[`!Sync`][__link28], it places **no** `Send`/`Sync` bound on `T`,
so it can share thread-affine values (e.g. `Rc<RefCell<T>>`) that [`Arc`][__link29]
cannot.

```rust
use multitude::Rc;

let arena = multitude::Arena::new();
let a: Rc<u64> = arena.alloc_rc(42);
let b = a.clone();
assert_eq!(*a, *b);
```

[`Box`][__link30] is a unique owner that provides `&mut T` access, like
[`alloc::boxed::Box`][__link31] but backed by the arena:

```rust
let arena = multitude::Arena::new();
let mut v = arena.alloc_box(vec![1, 2, 3]);
v.push(4);
assert_eq!(*v, vec![1, 2, 3, 4]);
drop(v); // The vec drop runs here, freeing its heap buffer.
```

Although [`Arena`][__link32] itself is `!Sync`, it is [`Send`][__link33] when its allocator is:
an arena with no outstanding borrows can be moved between threads. Eligible handles can also
be moved independently; an [`Alloc`][__link34] remains borrowed from its arena, while
escape-capable smart pointers do not. For cross-thread *sharing* of an
individual value, allocate an [`Arc`][__link35] and `.clone()` it across threads.

## Collections

[`Vec`][__link36], [`String`][__link37], and [`Utf16String`][__link38] are growable collections that live in
the arena.

Additionally, you can use an arena as
the allocator for any type from the [`allocator-api2`][__link39] ecosystem
(including `hashbrown::HashMap`).

```rust
use multitude::Arena;
use multitude::vec::{CollectIn, Vec};

let arena = Arena::new();

let mut v = arena.alloc_vec::<i32>();
for i in 0..5 {
    v.push(i);
}

// CollectIn trait for iterator collection.
let squares: Vec<i32, _> = (1..=5).map(|i| i * i).collect_in(&arena);
assert_eq!(squares.as_slice(), &[1, 4, 9, 16, 25]);
```

### Freezing

[`String`][__link40] and [`Vec`][__link41] are designed as **transient
builders** — mutable, growable handles meant to be used briefly and then frozen.

Once you’re done building, you can **freeze them** into immutable smart pointers:

* [`String::into_boxed_str`][__link42] →
  [`Box<str>`][__link43] (**8 bytes**, thin), or `Box::from(string)`.
  The freeze is generally **O(1)** and reuses the existing buffer, with an
  **O(n)** move only when the buffer cannot be frozen in place. (Like any
  [`Box`][__link44], it is `Send`/`Sync` only when the allocator `A` is.)
* [`Vec::into_boxed_slice`][__link45] →
  [`Box<[T]>`][__link46] (**8 bytes**, thin), or `Box::from(vec)`.
  This is also generally an **O(1)** in-place operation, with an **O(n)**
  element move as the fallback. (Like any [`Box`][__link47], it is `Send`/`Sync` only
  when `T` and the allocator `A` are.)
* `Arc::from(vec)` / `Arc::from(string)` → [`Arc<[T]>`][__link48] /
  [`Arc<str>`][__link49], the shared, reference-counted freeze
  (mirroring `std`’s `From<Vec<T>> for Arc<[T]>`).
  `Rc::from(vec)` / `Rc::from(string)` provides the non-atomic equivalent.
* [`Vec::leak`][__link50] → `&mut [T]` (or `&*v.leak()` for `&[T]`)
  borrowed for the arena’s lifetime. When `T` does not need drop, this freeze is
  **O(1) and allocation-free** — the existing buffer is reinterpreted in
  place. Unlike the `Box`/`Arc` freezes, the slice does not outlive the arena.

The `Vec` freeze also reclaims any unused capacity left in the
buffer when the conditions allow it, so those bytes become available
for the next allocation.

```rust
use multitude::{Arena, Box};

let arena = Arena::new();

// Build phase: 40-byte builder on 64-bit targets, alive briefly.
let mut builder = arena.alloc_string();
builder.push_str("hello, ");
builder.push_str("world");

// Freeze for storage: an 8-byte single-pointer smart pointer, normally in place.
let stored: Box<str> = builder.into_boxed_str();
assert_eq!(&*stored, "hello, world");
```

Use this pattern whenever you’d be storing many strings or slices
long-term — the per-pointer savings (8 bytes for both strings and
slices) add up quickly across millions of items.

### Maps and Sets

With the `hashbrown` Cargo feature, [`Arena`][__link51] can directly back
[`hashbrown`][__link52] collections via
[`Arena::alloc_hash_map`][__link53], [`Arena::alloc_hash_map_with_capacity`][__link54],
[`Arena::alloc_set`][__link55], and [`Arena::alloc_set_with_capacity`][__link56]. The returned
`HashMap` / `HashSet` store their entries in arena chunks.

```rust
use multitude::Arena;

let arena = Arena::new();

let mut map = arena.alloc_hash_map::<u32, &str>();
map.insert(1, "one");
assert_eq!(map.get(&1), Some(&"one"));

let mut set = arena.alloc_set::<u32>();
set.insert(7);
assert!(set.contains(&7));
```

## Strings

`multitude` provides a family of arena-resident string types in the
[`strings`][__link57] module. The model is the same one used for arbitrary
values elsewhere in the crate — bump-allocation backed by a per-chunk
refcount — but specialized for UTF-8 / UTF-16 text and a compact
single-pointer representation.

There are two roles a string type can play:

1. **Smart pointers (immutable / owned).** Compact handles to string
   data already stored in the arena. They use a single-pointer (8
   bytes on 64-bit) layout — half the size of `&str`. They differ in
   how sharing and mutability work:
   
   |UTF-8|UTF-16|Sharing|Mutable|Notes|
   |-----|------|-------|-------|-----|
   |[`Arc<str>`][__link58]|`Arc<Utf16Str>`|atomic refcount; `Clone`, `Send + Sync` with a compatible allocator|no|cross-thread sharing|
   |[`Rc<str>`][__link59]|`Rc<Utf16Str>`|non-atomic refcount; `Clone`, `!Send + !Sync`|no|single-thread sharing|
   |[`Box<str>`][__link60]|`Box<Utf16Str>`|unique owner; `Send + Sync` with a compatible allocator (not `Clone`)|yes|drops eagerly|
   
   Like the other arena smart pointers, they keep their owning chunk
   alive via a refcount, so they can outlive the [`Arena`][__link61] they came
   from.

1. **Builders (mutable, growable).** [`String`][__link62] and
   [`Utf16String`][__link63] are transient growable
   buffers — 40-byte structs on 64-bit targets carrying a data pointer,
   length, capacity, freeze state, and arena reference. You build them up with
   `push_str` / `push` / [`format!`][__link64] /
   [`format_utf16!`][__link65], then **freeze** them
   into one of the smart pointers above:
   
   |Builder|Freeze method|Result|
   |-------|-------------|------|
   |[`String`][__link66]|[`into_boxed_str`][__link67]|[`Box<str>`][__link68]|
   |[`Utf16String`][__link69]|[`into_boxed_utf16_str`][__link70]|`Box<Utf16Str>`|
   
   Both freezes generally reuse the buffer in place (O(1)) and reclaim any
   unused capacity when they can. Buffers that cannot freeze in place fall
   back to an O(n) element move. The resulting string smart pointers remain compact,
   single-pointer handles.

UTF-16 support requires the `utf16` Cargo feature. Strict (validated)
UTF-16 only — lone surrogates are rejected. The UTF-16 types
interoperate with `widestring::Utf16Str` / `widestring::Utf16String`
for I/O and FFI bridging. UTF-16 length and capacity are counted in
`u16` elements (matching `widestring::Utf16Str::len()`).

### Example: UTF-8

```rust
use multitude::Arena;
use multitude::Box;

let arena = Arena::new();

// Single-pointer immutable strings.
let s = arena.alloc_str_arc("hello, world");
assert_eq!(&*s, "hello, world");

// Build incrementally and freeze:
let mut b = arena.alloc_string();
b.push_str("abc");
b.push_str("123");
let frozen: Box<str> = b.into_boxed_str();
assert_eq!(&*frozen, "abc123");

// format!-style:
let name = "Alice";
let greeting = multitude::strings::format!(in &arena, "Hello, {name}!");
assert_eq!(&*greeting, "Hello, Alice!");
```

### Example: UTF-16

```rust
use multitude::Arena;
use widestring::utf16str;

let arena = Arena::new();

// From a validated &Utf16Str literal:
let s = arena.alloc_utf16_str_arc(utf16str!("hello, world"));
assert_eq!(&*s, utf16str!("hello, world"));

// Or transcode from a &str:
let s2 = arena.alloc_utf16_str_arc_from_str("hello");
assert_eq!(&*s2, utf16str!("hello"));

// Build incrementally and freeze:
let mut b = arena.alloc_utf16_string();
b.push_str(utf16str!("abc"));
b.push_from_str("123");
let frozen = b.into_boxed_utf16_str();
assert_eq!(&*frozen, utf16str!("abc123"));

// format!-style:
let name = "Alice";
let greeting = multitude::strings::format_utf16!(in &arena, "Hello, {name}!");
assert_eq!(greeting.as_utf16_str(), utf16str!("Hello, Alice!"));
```

## Arena-Aware Deserialization

With the `serde_json` feature, derive [`de::DeserializeIn`][__link71] on types that
should place owned fields in the arena, then call an [`Arena`][__link72] convenience
method:

```rust
#[derive(multitude::de::DeserializeIn)]
struct Request {
    id: u64,
    name: multitude::Box<str>,
}

let arena = multitude::Arena::new();
let request: Request = arena.deserialize_json(r#"{"id":7,"name":"Ada"}"#)?;
assert_eq!(request.name.as_ref(), "Ada");
```

Ordinary [`serde::Deserialize`][__link73] and [`de::DeserializeIn`][__link74] are independent;
see the [`de`][__link75] module for field compatibility, third-party types, ownership
choices, limits, and custom implementations.

## Building DSTs

With the `dst` Cargo feature enabled, [`Arena`][__link76] exposes
[`Arena::alloc_dst_arc`][__link77], [`Arena::alloc_dst_rc`][__link78], and
[`Arena::alloc_dst_box`][__link79] (along with fallible and pinned variants) for
constructing values whose layout is only known at runtime (custom
DSTs, fat pointers, trait objects).

Each of these takes a [`Layout`][__link80], a
pointer-metadata value (e.g. a slice length, a `DynMetadata`), and
a closure that initializes the buffer through a typed fat pointer.
For most users, the [`dst-factory`][__link81] companion crate is the
recommended high-level driver; the low-level interface looks like:

```rust
use core::alloc::Layout;

use multitude::Arena;

let arena = Arena::new();

// Allocate a 5-byte slice in the arena as a `Box<[u8]>`.
let layout = Layout::array::<u8>(5).unwrap();
let b: multitude::Box<[u8]> = unsafe {
    arena.alloc_dst_box::<[u8]>(layout, 5, |fat: *mut [u8]| {
        let p = fat.cast::<u8>();
        for i in 0..5 {
            p.add(i).write(i as u8);
        }
    })
};
assert_eq!(&*b, &[0, 1, 2, 3, 4]);
```

Separately, sized [`Box`][__link82], [`Rc`][__link83], and [`Arc`][__link84] values can be converted with
their `unsize` methods and [`coerce!`][__link85]. The `ptr_meta` dependency supplies
targets for `dyn Any` and `dyn Error`, including auto-trait combinations,
without the `dst` feature. Custom traits require `dst` and
`#[multitude::dst::pointee]`; see [`coerce!`][__link86] for the exact boundary.
Any resulting vtable-metadata pointee carries an additional handle word,
including a custom DST with a trait-object tail.

## Crate Features

|Feature|Description|
|-------|-----------|
|`std` *(default)*|Enables [`std::io::Write`][__link87] on [`Vec<u8>`][__link88] for use with `write!`, `std::io::copy`, `serde_json::to_writer`, and similar. Disable for `#![no_std]` environments (the crate still requires `alloc`).|
|`stats`|Enables runtime instrumentation counters returned by `Arena::stats`. Disable for the tightest allocation throughput when you don’t need observability.|
|`serde`|Adds `Serialize` impls for arena strings and vectors, plus arena-aware deserialization through [`de::DeserializeIn`][__link89] and [`Arena::deserialize`][__link90]. With `serde + utf16`, also adds serialization for the UTF-16 types (transcoded to UTF-8 on the wire).|
|`serde_json`|Implies `serde` and adds [`Arena::deserialize_json`][__link91] convenience methods with trailing-input checks and optional resource limits.|
|`dst`|Enables the `dst` module for constructing true dynamically-sized types and trait objects in the arena via the `Arena::alloc_dst_*` families.|
|`utf16`|Adds a parallel UTF-16 string surface (`Arc<Utf16Str>`, `Box<Utf16Str>`, [`Utf16String`][__link92], and [`format_utf16!`][__link93]) backed by the [`widestring`][__link94] crate. Lengths are counted in `u16` elements.|
|`zerocopy`|Provides [`ZerocopyView`][__link95] for safe zero-initialized allocation of types implementing [`zerocopy::FromZeros`][__link96]. Access via [`Arena::zerocopy()`][__link97].|
|`bytemuck`|Provides [`BytemuckView`][__link98] for safe zero-initialized allocation of types implementing [`bytemuck::Zeroable`][__link99]. Access via [`Arena::bytemuck()`][__link100].|
|`bytes`|Adds [`From`][__link101] conversions from [`Arc<[u8]>`][__link102] and [`Arc<str>`][__link103] into [`bytes::Bytes`][__link104], enabling zero-copy integration with the Tokio / Hyper async ecosystem.|
|`bytesbuf`|Implements [`bytesbuf::mem::Memory`][__link105] directly on [`Arena`][__link106], so that [`BytesBuf`][__link107] buffers can be backed by arena chunks. Implies `std`.|
|`hashbrown`|Lets [`Arena`][__link108] back [`hashbrown`][__link109] collections via [`Arena::alloc_hash_map`][__link110], [`Arena::alloc_hash_map_with_capacity`][__link111], [`Arena::alloc_set`][__link112], and [`Arena::alloc_set_with_capacity`][__link113].|


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/multitude">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGkYW0CYXSEG9dVcQv7gDzkG7VJ-FsdvgXwG4ndzbdWNuz6G6a5_GehYxcvYXKEG7Dw1reN_G4rG5DId0cdgdmjG8efwif7uA6zG4K8-qARz833YWSGgmhieXRlbXVja2YxLjI1LjKCZWJ5dGVzZjEuMTIuMYJoYnl0ZXNidWZlMC45LjCCaW11bHRpdHVkZWUwLjkuMIJlc2VyZGVnMS4wLjIyOYJoemVyb2NvcHlmMC44LjU2
 [__link0]: https://docs.rs/multitude/0.9.0/multitude/?search=alloc_handle::Alloc
 [__link1]: https://docs.rs/multitude/0.9.0/multitude/?search=arc::Arc
 [__link10]: https://docs.rs/multitude/0.9.0/multitude/?search=vec::Vec
 [__link100]: `Arena::bytemuck()`
 [__link101]: https://doc.rust-lang.org/stable/std/convert/trait.From.html
 [__link102]: https://docs.rs/multitude/0.9.0/multitude/?search=arc::Arc
 [__link103]: https://docs.rs/multitude/0.9.0/multitude/?search=arc::Arc
 [__link104]: https://docs.rs/bytes/1.12.1/bytes/?search=Bytes
 [__link105]: https://docs.rs/bytesbuf/0.9.0/bytesbuf/?search=mem::Memory
 [__link106]: https://docs.rs/multitude/0.9.0/multitude/?search=arena::Arena
 [__link107]: https://docs.rs/bytesbuf/0.9.0/bytesbuf/?search=BytesBuf
 [__link108]: https://docs.rs/multitude/0.9.0/multitude/?search=arena::Arena
 [__link109]: https://crates.io/crates/hashbrown
 [__link11]: https://docs.rs/multitude/0.9.0/multitude/?search=strings::format
 [__link110]: https://docs.rs/multitude/0.9.0/multitude/?search=arena::Arena::alloc_hash_map
 [__link111]: https://docs.rs/multitude/0.9.0/multitude/?search=arena::Arena::alloc_hash_map_with_capacity
 [__link112]: https://docs.rs/multitude/0.9.0/multitude/?search=arena::Arena::alloc_set
 [__link113]: https://docs.rs/multitude/0.9.0/multitude/?search=arena::Arena::alloc_set_with_capacity
 [__link12]: https://github.com/microsoft/oxidizer/blob/main/crates/multitude/docs/BUMPALO.md
 [__link13]: https://crates.io/crates/bumpalo
 [__link14]: https://github.com/microsoft/oxidizer/blob/main/crates/multitude/docs/PERF.md
 [__link15]: https://docs.rs/multitude/0.9.0/multitude/?search=alloc_handle::Alloc
 [__link16]: https://docs.rs/multitude/0.9.0/multitude/?search=r#box::Box
 [__link17]: https://docs.rs/multitude/0.9.0/multitude/?search=rc::Rc
 [__link18]: https://docs.rs/multitude/0.9.0/multitude/?search=arc::Arc
 [__link19]: https://docs.rs/multitude/0.9.0/multitude/?search=arena::Arena::alloc
 [__link2]: https://docs.rs/multitude/0.9.0/multitude/?search=rc::Rc
 [__link20]: https://docs.rs/multitude/0.9.0/multitude/?search=arena::Arena::alloc_box
 [__link21]: https://docs.rs/multitude/0.9.0/multitude/?search=arena::Arena::alloc_rc
 [__link22]: https://docs.rs/multitude/0.9.0/multitude/?search=arena::Arena::alloc_arc
 [__link23]: https://docs.rs/multitude/0.9.0/multitude/?search=alloc_handle::Alloc
 [__link24]: https://docs.rs/multitude/0.9.0/multitude/?search=arc::Arc
 [__link25]: https://docs.rs/multitude/0.9.0/multitude/?search=rc::Rc
 [__link26]: https://docs.rs/multitude/0.9.0/multitude/?search=arc::Arc
 [__link27]: https://doc.rust-lang.org/stable/std/marker/trait.Send.html
 [__link28]: https://doc.rust-lang.org/stable/std/marker/trait.Sync.html
 [__link29]: https://docs.rs/multitude/0.9.0/multitude/?search=arc::Arc
 [__link3]: https://docs.rs/multitude/0.9.0/multitude/?search=r#box::Box
 [__link30]: https://docs.rs/multitude/0.9.0/multitude/?search=r#box::Box
 [__link31]: https://doc.rust-lang.org/stable/alloc/?search=boxed::Box
 [__link32]: https://docs.rs/multitude/0.9.0/multitude/?search=arena::Arena
 [__link33]: https://doc.rust-lang.org/stable/std/marker/trait.Send.html
 [__link34]: https://docs.rs/multitude/0.9.0/multitude/?search=alloc_handle::Alloc
 [__link35]: https://docs.rs/multitude/0.9.0/multitude/?search=arc::Arc
 [__link36]: https://docs.rs/multitude/0.9.0/multitude/?search=vec::Vec
 [__link37]: https://docs.rs/multitude/0.9.0/multitude/?search=strings::String
 [__link38]: https://docs.rs/multitude/0.9.0/multitude/?search=strings::Utf16String
 [__link39]: https://crates.io/crates/allocator-api2
 [__link4]: https://docs.rs/multitude/0.9.0/multitude/?search=arc::Arc
 [__link40]: https://docs.rs/multitude/0.9.0/multitude/?search=strings::String
 [__link41]: https://docs.rs/multitude/0.9.0/multitude/?search=vec::Vec
 [__link42]: https://docs.rs/multitude/0.9.0/multitude/?search=strings::String::into_boxed_str
 [__link43]: https://docs.rs/multitude/0.9.0/multitude/?search=Box
 [__link44]: https://docs.rs/multitude/0.9.0/multitude/?search=r#box::Box
 [__link45]: https://docs.rs/multitude/0.9.0/multitude/?search=vec::Vec::into_boxed_slice
 [__link46]: https://docs.rs/multitude/0.9.0/multitude/?search=Box
 [__link47]: https://docs.rs/multitude/0.9.0/multitude/?search=r#box::Box
 [__link48]: https://docs.rs/multitude/0.9.0/multitude/?search=Arc
 [__link49]: https://docs.rs/multitude/0.9.0/multitude/?search=Arc
 [__link5]: https://docs.rs/multitude/0.9.0/multitude/?search=rc::Rc
 [__link50]: https://docs.rs/multitude/0.9.0/multitude/?search=vec::Vec::leak
 [__link51]: https://docs.rs/multitude/0.9.0/multitude/?search=arena::Arena
 [__link52]: https://crates.io/crates/hashbrown
 [__link53]: https://docs.rs/multitude/0.9.0/multitude/?search=arena::Arena::alloc_hash_map
 [__link54]: https://docs.rs/multitude/0.9.0/multitude/?search=arena::Arena::alloc_hash_map_with_capacity
 [__link55]: https://docs.rs/multitude/0.9.0/multitude/?search=arena::Arena::alloc_set
 [__link56]: https://docs.rs/multitude/0.9.0/multitude/?search=arena::Arena::alloc_set_with_capacity
 [__link57]: https://docs.rs/multitude/0.9.0/multitude/strings/index.html
 [__link58]: https://docs.rs/multitude/0.9.0/multitude/?search=Arc
 [__link59]: https://docs.rs/multitude/0.9.0/multitude/?search=Rc
 [__link6]: https://docs.rs/multitude/0.9.0/multitude/?search=r#box::Box
 [__link60]: https://docs.rs/multitude/0.9.0/multitude/?search=Box
 [__link61]: https://docs.rs/multitude/0.9.0/multitude/?search=arena::Arena
 [__link62]: https://docs.rs/multitude/0.9.0/multitude/?search=strings::String
 [__link63]: https://docs.rs/multitude/0.9.0/multitude/?search=strings::Utf16String
 [__link64]: https://docs.rs/multitude/0.9.0/multitude/?search=strings::format
 [__link65]: https://docs.rs/multitude/0.9.0/multitude/?search=strings::format_utf16
 [__link66]: https://docs.rs/multitude/0.9.0/multitude/?search=strings::String
 [__link67]: https://docs.rs/multitude/0.9.0/multitude/?search=strings::String::into_boxed_str
 [__link68]: https://docs.rs/multitude/0.9.0/multitude/?search=Box
 [__link69]: https://docs.rs/multitude/0.9.0/multitude/?search=strings::Utf16String
 [__link7]: https://docs.rs/multitude/0.9.0/multitude/?search=alloc_handle::Alloc
 [__link70]: https://docs.rs/multitude/0.9.0/multitude/?search=strings::Utf16String::into_boxed_utf16_str
 [__link71]: https://docs.rs/multitude/0.9.0/multitude/?search=de::DeserializeIn
 [__link72]: https://docs.rs/multitude/0.9.0/multitude/?search=arena::Arena
 [__link73]: https://docs.rs/serde/1.0.229/serde/?search=Deserialize
 [__link74]: https://docs.rs/multitude/0.9.0/multitude/?search=de::DeserializeIn
 [__link75]: https://docs.rs/multitude/0.9.0/multitude/de/index.html
 [__link76]: https://docs.rs/multitude/0.9.0/multitude/?search=arena::Arena
 [__link77]: https://docs.rs/multitude/0.9.0/multitude/?search=arena::Arena::alloc_dst_arc
 [__link78]: https://docs.rs/multitude/0.9.0/multitude/?search=arena::Arena::alloc_dst_rc
 [__link79]: https://docs.rs/multitude/0.9.0/multitude/?search=arena::Arena::alloc_dst_box
 [__link8]: https://docs.rs/multitude/0.9.0/multitude/?search=strings::String
 [__link80]: https://doc.rust-lang.org/stable/core/?search=alloc::Layout
 [__link81]: https://crates.io/crates/dst-factory
 [__link82]: https://docs.rs/multitude/0.9.0/multitude/?search=r#box::Box
 [__link83]: https://docs.rs/multitude/0.9.0/multitude/?search=rc::Rc
 [__link84]: https://docs.rs/multitude/0.9.0/multitude/?search=arc::Arc
 [__link85]: `coerce!`
 [__link86]: `coerce!`
 [__link87]: https://doc.rust-lang.org/stable/std/?search=io::Write
 [__link88]: https://docs.rs/multitude/0.9.0/multitude/?search=vec::Vec
 [__link89]: https://docs.rs/multitude/0.9.0/multitude/?search=de::DeserializeIn
 [__link9]: https://docs.rs/multitude/0.9.0/multitude/?search=strings::Utf16String
 [__link90]: https://docs.rs/multitude/0.9.0/multitude/?search=arena::Arena::deserialize
 [__link91]: https://docs.rs/multitude/0.9.0/multitude/?search=arena::Arena::deserialize_json
 [__link92]: https://docs.rs/multitude/0.9.0/multitude/?search=strings::Utf16String
 [__link93]: https://docs.rs/multitude/0.9.0/multitude/?search=strings::format_utf16
 [__link94]: https://crates.io/crates/widestring
 [__link95]: https://docs.rs/multitude/0.9.0/multitude/?search=zerocopy::ZerocopyView
 [__link96]: https://docs.rs/zerocopy/0.8.56/zerocopy/?search=FromZeros
 [__link97]: `Arena::zerocopy()`
 [__link98]: https://docs.rs/multitude/0.9.0/multitude/?search=bytemuck::BytemuckView
 [__link99]: https://docs.rs/bytemuck/1.25.2/bytemuck/?search=Zeroable
