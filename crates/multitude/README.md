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

`multitude` is a bump allocator for applications with phase-oriented lifetimes.

Groups of related allocations live and die together. Service request handling and parsers are two examples of this pattern which usually
benefit from a bump allocator.

`multitude` works by accumulating large chunks of memory allocated from the system and then carving out smaller pieces of it for application use
using a fast bump allocation strategy, which is considerably faster than allocating from the system. The downside however is that the individual allocations
can’t be freed separately. Instead, memory is reclaimed and returned to the system in bulk when the entire arena is dropped.

## Why Another Bump Allocator?

The Rust ecosystem has a few bump allocators, the most popular being [`bumpalo`][__link0].
`multitude` uses a different implementation strategy and has a richer API surface making it suitable for more
use cases. The main features that set `multitude` apart are:

1. **Flexibility.** Four allocation styles coexist in the same arena: the
   arena-lifetime owning handle [`Alloc<T>`][__link1] plus three escape-capable
   smart pointers — the atomic [`Arc`][__link2], the non-atomic single-thread [`Rc`][__link3],
   and the unique-owner [`Box`][__link4] — each available for sized `T`, `str`, and
   `[T]`. See the [comparison table](#flexibility) for how they differ.

1. **Early Reclamation.** In many situations, `multitude` can reclaim memory from individual chunks as soon as their reference counts drop to zero,
   without waiting for the entire arena to be dropped. This allows for more efficient memory usage in long-running arenas with many short-lived allocations.

1. **Smart Pointers Can Outlive the Arena.** Some of the smart pointers produced by `multitude` can keep their owning chunk alive even after the arena itself has been dropped,
   allowing for more flexible memory management and longer-lived data structures.

1. **Drop Support.** `multitude` automatically runs `Drop` for allocated values at the appropriate time.

1. **Uniformly Thin Smart Pointers.** `multitude`’s escape-capable smart
   pointers — [`Arc<T>`][__link5], [`Rc<T>`][__link6], and [`Box<T>`][__link7] — are
   **8 bytes** on 64-bit for *every* `T`, even DSTs like `str` and `[T]`
   (the metadata lives in a chunk prefix). The arena-lifetime
   [`Alloc<T>`][__link8] handle is a single word for sized `T`; for `str` /
   `[T]` it is a fat reference (pointer + length), which costs nothing extra
   since it never escapes the arena and isn’t stored at scale.

1. **Efficient Mutable Strings and Vectors.** `multitude` provides [`String`][__link9], [`Utf16String`][__link10] and [`Vec`][__link11] which are growable collections that live in the arena.

1. **Dynamically-Sized Types.** `multitude` supports dynamically-sized types (DSTs) like slices and strings, allowing you to allocate and manage them in the
   arena with the same flexibility as sized types. The [`dst-factory`][__link12] crate is a great companion for building DSTs in the arena.

1. **First Class `serde` Support.**. `multitude` lets you deserialize data directly into
   arena-backed memory.

1. **`format!`-style Macro.** `multitude` includes a [`format!`][__link13]-style macro that allows you to create formatted strings directly in the arena, avoiding intermediate allocations and copies.

1. **UTF-16 Support.** With the `utf16` Cargo feature, `multitude` provides a parallel set of arena-resident UTF-16 string types
   (`Arc<Utf16Str>`, `Box<Utf16Str>`, [`Utf16String`][__link14]) and a [`format_utf16!`][__link15] macro for FFI / Windows / JS-engine
   interop without per-call transcoding at every boundary.

1. **`#![no_std]` Support.** `multitude` can be used in `#![no_std]` environments, making it suitable for embedded systems and other resource-constrained contexts.

See [`BUMPALO.md`][__link16]
for a feature-by-feature comparison with [`bumpalo`][__link17].

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
arena, whether ownership is unique or shared, and what (if any) per-handle
reference count they pay. Each is available for sized `T`, `str`, and `[T]`
(and, behind the `dst` feature, arbitrary DSTs).

||[`Alloc<T>`][__link18]|[`Box<T>`][__link19]|[`Rc<T>`][__link20]|[`Arc<T>`][__link21]|
|-|:--------:|:------:|:-----:|:------:|
|**Constructor family**|[`alloc`][__link22]|[`alloc_box`][__link23]|[`alloc_rc`][__link24]|[`alloc_arc`][__link25]|
|**Ownership**|unique|unique|shared (`Clone`)|shared (`Clone`)|
|**`&mut T` access**|✅|✅|❌|❌|
|**Can outlive the arena**|❌|✅|✅|✅|
|**Per-handle reference count**|none|none|non-atomic|atomic|
|**Cross-thread *sharing***|❌|❌|❌ (`!Send`)|✅ (`T: Send + Sync`)|
|**Width (64-bit)**|1 word (sized); fat ref for DSTs|8 bytes|8 bytes|8 bytes|

The cheapest option is [`Alloc<T>`][__link26]: an owning handle whose lifetime
is tied to the arena — a single word for sized `T` (a fat pointer+length
reference for `str` / `[T]`). It pays no reference count and cannot
outlive the arena, but gives mutable access and runs the destructor when it
is dropped — the fastest way to allocate when the lifetime constraint is
tolerable.

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
pointers. They behave like the like-named `std` types but are uniformly
**8-byte thin pointers** (even for DSTs) addressing storage inside a chunk,
and they keep that chunk alive until the last handle drops.

[`Arc`][__link27] is reference-counted and shareable across threads:

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

[`Rc`][__link28] is the cheaper single-thread sibling of [`Arc`][__link29]: its reference count
is non-atomic, so `clone`/`drop` are cheaper and `str` / `[u8]` pack slightly
tighter. Being [`!Send`][__link30]/[`!Sync`][__link31], it places **no** `Send`/`Sync`
bound on `T`, so it can share thread-affine values (e.g. `Rc<RefCell<T>>`)
that [`Arc`][__link32] cannot.

```rust
use multitude::Rc;

let arena = multitude::Arena::new();
let a: Rc<u64> = arena.alloc_rc(42);
let b = a.clone();
assert_eq!(*a, *b);
```

[`Box`][__link33] is a unique owner that provides `&mut T` access, like
[`alloc::boxed::Box`][__link34] but backed by the arena:

```rust
let arena = multitude::Arena::new();
let mut v = arena.alloc_box(vec![1, 2, 3]);
v.push(4);
assert_eq!(*v, vec![1, 2, 3, 4]);
drop(v); // The vec drop runs here, freeing its heap buffer.
```

Although [`Arena`][__link35] itself is `!Sync`, it is [`Send`][__link36]: an arena — along with
any in-flight [`Alloc`][__link37] handles and smart pointers — can be moved between
threads. For cross-thread *sharing* of an individual value, allocate an
[`Arc`][__link38] and `.clone()` it across threads.

## Collections

[`Vec`][__link39], [`String`][__link40], and [`Utf16String`][__link41] are growable collections that live in
the arena.

Additionally, you can use an arena as
the allocator for any type from the [`allocator-api2`][__link42] ecosystem
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

[`String`][__link43] and [`Vec`][__link44] are designed as **transient
builders** — mutable, growable handles meant to be used briefly and then frozen.

Once you’re done building, you can **freeze them** into immutable smart pointers:

* [`String::into_boxed_str`][__link45] →
  [`Box<str>`][__link46] (**8 bytes**, thin), or `Box::from(string)`.
  The freeze is **O(n)** — it copies the bytes into a compact allocation
  that can outlive the arena. (Like any [`Box`][__link47], it is `Send`/`Sync` only
  when the allocator `A` is.)
* [`Vec::into_boxed_slice`][__link48] →
  [`Box<[T]>`][__link49] (**8 bytes**, thin), or `Box::from(vec)`.
  The freeze is **O(n)** — it moves the elements into a fresh compact
  allocation that can outlive the arena. (Like any [`Box`][__link50], it is
  `Send`/`Sync` only when `T` and the allocator `A` are.)
* `Arc::from(vec)` / `Arc::from(string)` → [`Arc<[T]>`][__link51] /
  [`Arc<str>`][__link52], the shared, reference-counted freeze
  (mirroring `std`’s `From<Vec<T>> for Arc<[T]>`).
* [`Vec::leak`][__link53] → `&mut [T]` (or `&*v.leak()` for `&[T]`)
  borrowed for the arena’s lifetime. For `T: !Drop`, this freeze is
  **O(1) and allocation-free** — the existing buffer is reinterpreted in
  place. Unlike the `Box`/`Arc` freezes, the slice does not outlive the arena.

The `Vec` freeze also reclaims any unused capacity left in the
buffer when the conditions allow it, so those bytes become available
for the next allocation.

```rust
use multitude::{Arena, Box};

let arena = Arena::new();

// Build phase: 32-byte builder, alive briefly.
let mut builder = arena.alloc_string();
builder.push_str("hello, ");
builder.push_str("world");

// Freeze for storage: 8-byte single-pointer smart pointer. O(n) — copies the bytes.
let stored: Box<str> = builder.into_boxed_str();
assert_eq!(&*stored, "hello, world");
```

Use this pattern whenever you’d be storing many strings or slices
long-term — the per-pointer savings (8 bytes for both strings and
slices) add up quickly across millions of items.

### Maps and Sets

With the `hashbrown` Cargo feature, [`Arena`][__link54] can directly back
[`hashbrown`][__link55] collections via
[`Arena::alloc_hash_map`][__link56], [`Arena::alloc_hash_map_with_capacity`][__link57],
[`Arena::alloc_set`][__link58], and [`Arena::alloc_set_with_capacity`][__link59]. The returned
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
[`strings`][__link60] module. The model is the same one used for arbitrary
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
   |[`Arc<str>`][__link61]|`Arc<Utf16Str>`|atomic refcount; `Clone`, `Send + Sync`|no|cross-thread sharing|
   |[`Box<str>`][__link62]|`Box<Utf16Str>`|unique owner; `Send + Sync` (not `Clone`)|yes|drops eagerly|
   
   Like the other arena smart pointers, they keep their owning chunk
   alive via a refcount, so they can outlive the [`Arena`][__link63] they came
   from.

1. **Builders (mutable, growable).** [`String`][__link64] and
   [`Utf16String`][__link65] are transient growable
   buffers — small structs (32 bytes) carrying a data pointer +
   length + capacity + arena reference. You build them up with
   `push_str` / `push` / [`format!`][__link66] /
   [`format_utf16!`][__link67], then **freeze** them
   into one of the smart pointers above:
   
   |Builder|Freeze method|Result|
   |-------|-------------|------|
   |[`String`][__link68]|[`into_boxed_str`][__link69]|[`Box<str>`][__link70]|
   |[`Utf16String`][__link71]|[`into_boxed_utf16_str`][__link72]|`Box<Utf16Str>`|
   
   The UTF-16 freeze reuses the buffer in place (O(1)) and reclaims any
   unused capacity when it can. The UTF-8 freeze copies the bytes (O(n))
   into a compact allocation, so [`Box<str>`][__link73] stays a single,
   `Send`-safe pointer.

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

With the `serde_json` feature, derive [`de::DeserializeIn`][__link74] on types that
should place owned fields in the arena, then call an [`Arena`][__link75] convenience
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

Ordinary [`serde::Deserialize`][__link76] and [`de::DeserializeIn`][__link77] are independent;
see the [`de`][__link78] module for field compatibility, third-party types, ownership
choices, limits, and custom implementations.

## Building DSTs

With the `dst` Cargo feature enabled, [`Arena`][__link79] exposes
[`Arena::alloc_dst_arc`][__link80] and
[`Arena::alloc_dst_box`][__link81] (and their `try_*` siblings) for
constructing values whose layout is only known at runtime (custom
DSTs, fat pointers, trait objects).

Each of these takes a [`Layout`][__link82], a
pointer-metadata value (e.g. a slice length, a `DynMetadata`), and
a closure that initializes the buffer through a typed fat pointer.
For most users, the [`dst-factory`][__link83] companion crate is the
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

The same feature also enables eight `Arena::alloc_slice_*_box`
methods that produce `Box<[T]>` directly (mirroring the
existing `_arc` slice methods).

## Crate Features

|Feature|Description|
|-------|-----------|
|`std` *(default)*|Enables [`std::io::Write`][__link84] on [`Vec<u8>`][__link85] for use with `write!`, `std::io::copy`, `serde_json::to_writer`, and similar. Disable for `#![no_std]` environments (the crate still requires `alloc`).|
|`stats`|Enables runtime instrumentation counters returned by `Arena::stats`. Disable for the tightest allocation throughput when you don’t need observability.|
|`serde`|Adds `Serialize` impls for arena strings and vectors, plus arena-aware deserialization through [`de::DeserializeIn`][__link86] and [`Arena::deserialize`][__link87]. With `serde + utf16`, also adds serialization for the UTF-16 types (transcoded to UTF-8 on the wire).|
|`serde_json`|Implies `serde` and adds [`Arena::deserialize_json`][__link88] convenience methods with trailing-input checks and optional resource limits.|
|`dst`|Enables the `dst` module for constructing true dynamically-sized types and trait objects in the arena via [`Arena::alloc_dst_arc`][__link89] / [`Arena::alloc_dst_box`][__link90], plus eight `Arena::alloc_slice_*_box` methods.|
|`utf16`|Adds a parallel UTF-16 string surface (`Arc<Utf16Str>`, `Box<Utf16Str>`, [`Utf16String`][__link91], and [`format_utf16!`][__link92]) backed by the [`widestring`][__link93] crate. Lengths are counted in `u16` elements.|
|`zerocopy`|Provides [`ZerocopyView`][__link94] for safe zero-initialized allocation of types implementing [`zerocopy::FromZeros`][__link95]. Access via [`Arena::zerocopy()`][__link96].|
|`bytemuck`|Provides [`BytemuckView`][__link97] for safe zero-initialized allocation of types implementing [`bytemuck::Zeroable`][__link98]. Access via [`Arena::bytemuck()`][__link99].|
|`bytes`|Adds [`From`][__link100] conversions from [`Arc<[u8]>`][__link101] and [`Arc<str>`][__link102] into [`bytes::Bytes`][__link103], enabling zero-copy integration with the Tokio / Hyper async ecosystem.|
|`bytesbuf`|Implements [`bytesbuf::mem::Memory`][__link104] directly on [`Arena`][__link105], so that [`BytesBuf`][__link106] buffers can be backed by arena chunks. Implies `std`.|
|`hashbrown`|Lets [`Arena`][__link107] back [`hashbrown`][__link108] collections via [`Arena::alloc_hash_map`][__link109], [`Arena::alloc_hash_map_with_capacity`][__link110], [`Arena::alloc_set`][__link111], and [`Arena::alloc_set_with_capacity`][__link112].|


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/multitude">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjJhdIQb11VxC_uAPOQbtUn4Wx2-BfAbid3Nt1Y27Pobprn8Z6FjFy9hYvRhcoQbJ_UnBmWjxVIbTDzDZ2HPGH4bfWz8uL9mSOwb6YHGY6ygXDRhZIaCaGJ5dGVtdWNrZjEuMjUuMIJlYnl0ZXNmMS4xMi4wgmhieXRlc2J1ZmUwLjcuMIJpbXVsdGl0dWRlZTAuNi4zgmVzZXJkZWcxLjAuMjI4gmh6ZXJvY29weWYwLjguNTI
 [__link0]: https://crates.io/crates/bumpalo
 [__link1]: https://docs.rs/multitude/0.6.3/multitude/?search=Alloc
 [__link10]: https://docs.rs/multitude/0.6.3/multitude/?search=strings::Utf16String
 [__link100]: https://doc.rust-lang.org/stable/std/convert/trait.From.html
 [__link101]: https://docs.rs/multitude/0.6.3/multitude/?search=Arc
 [__link102]: https://docs.rs/multitude/0.6.3/multitude/?search=Arc
 [__link103]: https://docs.rs/bytes/1.12.0/bytes/?search=Bytes
 [__link104]: https://docs.rs/bytesbuf/0.7.0/bytesbuf/?search=mem::Memory
 [__link105]: https://docs.rs/multitude/0.6.3/multitude/?search=Arena
 [__link106]: https://docs.rs/bytesbuf/0.7.0/bytesbuf/?search=BytesBuf
 [__link107]: https://docs.rs/multitude/0.6.3/multitude/?search=Arena
 [__link108]: https://crates.io/crates/hashbrown
 [__link109]: https://docs.rs/multitude/0.6.3/multitude/?search=Arena::alloc_hash_map
 [__link11]: https://docs.rs/multitude/0.6.3/multitude/?search=vec::Vec
 [__link110]: https://docs.rs/multitude/0.6.3/multitude/?search=Arena::alloc_hash_map_with_capacity
 [__link111]: https://docs.rs/multitude/0.6.3/multitude/?search=Arena::alloc_set
 [__link112]: https://docs.rs/multitude/0.6.3/multitude/?search=Arena::alloc_set_with_capacity
 [__link12]: https://crates.io/crates/dst-factory
 [__link13]: https://docs.rs/multitude/0.6.3/multitude/?search=strings::format
 [__link14]: https://docs.rs/multitude/0.6.3/multitude/?search=strings::Utf16String
 [__link15]: https://docs.rs/multitude/0.6.3/multitude/?search=strings::format_utf16
 [__link16]: https://github.com/microsoft/oxidizer/blob/main/crates/multitude/docs/BUMPALO.md
 [__link17]: https://crates.io/crates/bumpalo
 [__link18]: https://docs.rs/multitude/0.6.3/multitude/?search=Alloc
 [__link19]: https://docs.rs/multitude/0.6.3/multitude/?search=Box
 [__link2]: https://docs.rs/multitude/0.6.3/multitude/?search=Arc
 [__link20]: https://docs.rs/multitude/0.6.3/multitude/?search=Rc
 [__link21]: https://docs.rs/multitude/0.6.3/multitude/?search=Arc
 [__link22]: https://docs.rs/multitude/0.6.3/multitude/?search=Arena::alloc
 [__link23]: https://docs.rs/multitude/0.6.3/multitude/?search=Arena::alloc_box
 [__link24]: https://docs.rs/multitude/0.6.3/multitude/?search=Arena::alloc_rc
 [__link25]: https://docs.rs/multitude/0.6.3/multitude/?search=Arena::alloc_arc
 [__link26]: https://docs.rs/multitude/0.6.3/multitude/?search=Alloc
 [__link27]: https://docs.rs/multitude/0.6.3/multitude/?search=Arc
 [__link28]: https://docs.rs/multitude/0.6.3/multitude/?search=Rc
 [__link29]: https://docs.rs/multitude/0.6.3/multitude/?search=Arc
 [__link3]: https://docs.rs/multitude/0.6.3/multitude/?search=Rc
 [__link30]: https://doc.rust-lang.org/stable/std/marker/trait.Send.html
 [__link31]: https://doc.rust-lang.org/stable/std/marker/trait.Sync.html
 [__link32]: https://docs.rs/multitude/0.6.3/multitude/?search=Arc
 [__link33]: https://docs.rs/multitude/0.6.3/multitude/?search=Box
 [__link34]: https://doc.rust-lang.org/stable/alloc/?search=boxed::Box
 [__link35]: https://docs.rs/multitude/0.6.3/multitude/?search=Arena
 [__link36]: https://doc.rust-lang.org/stable/std/marker/trait.Send.html
 [__link37]: https://docs.rs/multitude/0.6.3/multitude/?search=Alloc
 [__link38]: https://docs.rs/multitude/0.6.3/multitude/?search=Arc
 [__link39]: https://docs.rs/multitude/0.6.3/multitude/?search=vec::Vec
 [__link4]: https://docs.rs/multitude/0.6.3/multitude/?search=Box
 [__link40]: https://docs.rs/multitude/0.6.3/multitude/?search=strings::String
 [__link41]: https://docs.rs/multitude/0.6.3/multitude/?search=strings::Utf16String
 [__link42]: https://crates.io/crates/allocator-api2
 [__link43]: https://docs.rs/multitude/0.6.3/multitude/?search=strings::String
 [__link44]: https://docs.rs/multitude/0.6.3/multitude/?search=vec::Vec
 [__link45]: https://docs.rs/multitude/0.6.3/multitude/?search=strings::String::into_boxed_str
 [__link46]: https://docs.rs/multitude/0.6.3/multitude/?search=Box
 [__link47]: https://docs.rs/multitude/0.6.3/multitude/?search=Box
 [__link48]: https://docs.rs/multitude/0.6.3/multitude/?search=vec::Vec::into_boxed_slice
 [__link49]: https://docs.rs/multitude/0.6.3/multitude/?search=Box
 [__link5]: https://docs.rs/multitude/0.6.3/multitude/?search=Arc
 [__link50]: https://docs.rs/multitude/0.6.3/multitude/?search=Box
 [__link51]: https://docs.rs/multitude/0.6.3/multitude/?search=Arc
 [__link52]: https://docs.rs/multitude/0.6.3/multitude/?search=Arc
 [__link53]: https://docs.rs/multitude/0.6.3/multitude/?search=vec::Vec::leak
 [__link54]: https://docs.rs/multitude/0.6.3/multitude/?search=Arena
 [__link55]: https://crates.io/crates/hashbrown
 [__link56]: https://docs.rs/multitude/0.6.3/multitude/?search=Arena::alloc_hash_map
 [__link57]: https://docs.rs/multitude/0.6.3/multitude/?search=Arena::alloc_hash_map_with_capacity
 [__link58]: https://docs.rs/multitude/0.6.3/multitude/?search=Arena::alloc_set
 [__link59]: https://docs.rs/multitude/0.6.3/multitude/?search=Arena::alloc_set_with_capacity
 [__link6]: https://docs.rs/multitude/0.6.3/multitude/?search=Rc
 [__link60]: https://docs.rs/multitude/0.6.3/multitude/strings/index.html
 [__link61]: https://docs.rs/multitude/0.6.3/multitude/?search=Arc
 [__link62]: https://docs.rs/multitude/0.6.3/multitude/?search=Box
 [__link63]: https://docs.rs/multitude/0.6.3/multitude/?search=Arena
 [__link64]: https://docs.rs/multitude/0.6.3/multitude/?search=strings::String
 [__link65]: https://docs.rs/multitude/0.6.3/multitude/?search=strings::Utf16String
 [__link66]: https://docs.rs/multitude/0.6.3/multitude/?search=strings::format
 [__link67]: https://docs.rs/multitude/0.6.3/multitude/?search=strings::format_utf16
 [__link68]: https://docs.rs/multitude/0.6.3/multitude/?search=strings::String
 [__link69]: https://docs.rs/multitude/0.6.3/multitude/?search=strings::String::into_boxed_str
 [__link7]: https://docs.rs/multitude/0.6.3/multitude/?search=Box
 [__link70]: https://docs.rs/multitude/0.6.3/multitude/?search=Box
 [__link71]: https://docs.rs/multitude/0.6.3/multitude/?search=strings::Utf16String
 [__link72]: https://docs.rs/multitude/0.6.3/multitude/?search=strings::Utf16String::into_boxed_utf16_str
 [__link73]: https://docs.rs/multitude/0.6.3/multitude/?search=Box
 [__link74]: https://docs.rs/multitude/0.6.3/multitude/?search=de::DeserializeIn
 [__link75]: https://docs.rs/multitude/0.6.3/multitude/?search=Arena
 [__link76]: https://docs.rs/serde/1.0.228/serde/?search=Deserialize
 [__link77]: https://docs.rs/multitude/0.6.3/multitude/?search=de::DeserializeIn
 [__link78]: https://docs.rs/multitude/0.6.3/multitude/de/index.html
 [__link79]: https://docs.rs/multitude/0.6.3/multitude/?search=Arena
 [__link8]: https://docs.rs/multitude/0.6.3/multitude/?search=Alloc
 [__link80]: https://docs.rs/multitude/0.6.3/multitude/?search=Arena::alloc_dst_arc
 [__link81]: https://docs.rs/multitude/0.6.3/multitude/?search=Arena::alloc_dst_box
 [__link82]: https://doc.rust-lang.org/stable/core/?search=alloc::Layout
 [__link83]: https://crates.io/crates/dst-factory
 [__link84]: https://doc.rust-lang.org/stable/std/?search=io::Write
 [__link85]: https://docs.rs/multitude/0.6.3/multitude/?search=vec::Vec
 [__link86]: https://docs.rs/multitude/0.6.3/multitude/?search=de::DeserializeIn
 [__link87]: https://docs.rs/multitude/0.6.3/multitude/?search=Arena::deserialize
 [__link88]: https://docs.rs/multitude/0.6.3/multitude/?search=Arena::deserialize_json
 [__link89]: https://docs.rs/multitude/0.6.3/multitude/?search=Arena::alloc_dst_arc
 [__link9]: https://docs.rs/multitude/0.6.3/multitude/?search=strings::String
 [__link90]: https://docs.rs/multitude/0.6.3/multitude/?search=Arena::alloc_dst_box
 [__link91]: https://docs.rs/multitude/0.6.3/multitude/?search=strings::Utf16String
 [__link92]: https://docs.rs/multitude/0.6.3/multitude/?search=strings::format_utf16
 [__link93]: https://crates.io/crates/widestring
 [__link94]: https://docs.rs/multitude/0.6.3/multitude/?search=zerocopy::ZerocopyView
 [__link95]: https://docs.rs/zerocopy/0.8.52/zerocopy/?search=FromZeros
 [__link96]: https://docs.rs/multitude/0.6.3/multitude/?search=Arena::zerocopy
 [__link97]: https://docs.rs/multitude/0.6.3/multitude/?search=bytemuck::BytemuckView
 [__link98]: https://docs.rs/bytemuck/1.25.0/bytemuck/?search=Zeroable
 [__link99]: https://docs.rs/multitude/0.6.3/multitude/?search=Arena::bytemuck
