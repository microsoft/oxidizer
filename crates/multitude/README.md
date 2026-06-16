<div align="center">
 <img src="./logo.png" alt="Multitude Logo" width="96">

# Multitude

[![crate.io](https://img.shields.io/crates/v/multitude.svg)](https://crates.io/crates/multitude)
[![docs.rs](https://docs.rs/multitude/badge.svg)](https://docs.rs/multitude)
[![MSRV](https://img.shields.io/crates/msrv/multitude)](https://crates.io/crates/multitude)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](../../LICENSE)
<a href="../.."><img src="../../logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Fast and flexible arena-based bump allocator.

`multitude` is an arena-based bump allocator designed to improve the performance of applications that have **phase-oriented logic**, which
is when groups of related allocations live and die together. Service request handling and parsers are two examples of this pattern which usually
benefit from a bump allocator.

`multitude` works by accumulating large chunks of memory allocated from the system and then carving out smaller pieces of it for application use
using a fast bump allocation strategy, which is considerably faster than allocating from the system. The downside however is that the individual allocations
can’t be freed separately. Instead, memory is reclaimed and returned to the system in bulk when the entire arena is dropped.

## Why Another Bump Allocator?

The Rust ecosystem has a few bump allocators, the most popular being [`bumpalo`][__link0].
`multitude` uses a different implementation strategy and has a richer API surface making it suitable for more
use cases. The main features that set `multitude` apart are:

1. **Flexibility.** `multitude` provides multiple allocation styles, all of
   which can coexist in the same arena:
   
   * Mutable references with lifetimes tied to the arena (`&mut T`,
     `&mut str`, `&mut [T]`).
   * Atomic reference-counted smart pointers ([`Arc`][__link1], [`Arc<str>`][__link2], [`Arc<[T]>`][__link3])
     for cross-thread sharing.
   * Owned, mutable smart pointers ([`Box`][__link4], [`Box<str>`][__link5], [`Box<[T]>`][__link6]).
1. **Early Reclamation.** In many situations, `multitude` can reclaim memory from individual chunks as soon as their reference counts drop to zero,
   without waiting for the entire arena to be dropped. This allows for more efficient memory usage in long-running arenas with many short-lived allocations.

1. **Smart Pointers Can Outlive the Arena.** The reference-counted smart pointers produced by `multitude` can keep their owning chunk alive even after the arena itself has been dropped,
   allowing for more flexible memory management and longer-lived data structures.

1. **Drop Support.** `multitude` automatically runs `Drop` for allocated values at the appropriate time.

1. **Uniformly Thin Smart Pointers.** `multitude`’s [`Arc<T>`][__link7] and [`Box<T>`][__link8] are **8 bytes** on 64-bit
   for *every* `T`, including DST `T` such as `str`, `[U]`, and `dyn Trait`. DST metadata (slice length, vtable)
   is stored unaligned in the chunk prefix, so `Vec<Arc<str>>` and similar collections are 2× denser than `Vec<std::sync::Arc<str>>`.

1. **Efficient Mutable Strings and Vectors.** `multitude` provides [`String`][__link9] and [`Vec`][__link10] which are growable collections that live in the arena.

1. **Dynamically-Sized Types.** `multitude` supports dynamically-sized types (DSTs) like slices and strings, allowing you to allocate and manage them in the
   arena with the same flexibility as sized types. The [`dst-factory`][__link11] crate is a great companion for building DSTs in the arena.

1. **`format!`-style Macro.** `multitude` includes a [`format!`][__link12]-style macro that allows you to create formatted strings directly in the arena, avoiding intermediate allocations and copies.

1. **UTF-16 Support.** With the `utf16` Cargo feature, `multitude` provides a parallel set of arena-resident UTF-16 string types
   ([`ArcUtf16Str`][__link13], [`BoxUtf16Str`][__link14], [`Utf16String`][__link15]) and a [`format_utf16!`][__link16] macro for FFI / Windows / JS-engine
   interop without per-call transcoding at every boundary.

1. **`#![no_std]` Support.** `multitude` can be used in `#![no_std]` environments, making it suitable for embedded systems and other resource-constrained contexts.

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

`multitude` supports a variety of ways to allocate data and track it over time.

### Simple References

The simplest use of the arena is to get plain mutable references. The lifetime of those references is then tied
to the arena’s own lifetime.

```rust
let arena = multitude::Arena::new();
let x: &mut u32 = arena.alloc(42);
let y: &mut u32 = arena.alloc(100);
*x += 1;
*y += 1;
assert_eq!(*x, 43);
assert_eq!(*y, 101);

// Strings and slices too:
let s: &mut str = arena.alloc_str("hello");
let v: &mut [i32] = arena.alloc_slice_copy(&[1, 2, 3]);
```

These references can’t outlive the arena, which limits their use. But they are the fastest and
most efficient way to allocate from the arena, so if the lifetime constraints are tolerable, simple
references are the way to go.

### Smart Pointers

Smart pointers ([`Arc`][__link17], [`Box`][__link18] and their `str` variations) work in a way similar to the like-named types
in the standard library, except that they reference addresses within an arena.

```rust
use multitude::Arc;

struct Point {
    x: f64,
    y: f64,
}

let p: Arc<Point> = {
    let arena = multitude::Arena::new();
    arena.alloc_arc(Point { x: 3.0, y: 4.0 })
    // arena dropped here
};
assert_eq!(p.x, 3.0);
```

Although [`Arena`][__link19] itself is `!Sync`, it is [`Send`][__link20]: an arena —
along with any in-flight references and smart pointers — can be
moved between threads. For cross-thread *sharing*, allocate
[`Arc`][__link21]-family smart pointers (e.g. [`Arc<u64>`][__link22], [`Arc<str>`][__link23])
and `.clone()` them across threads.

```rust
let arena = multitude::Arena::new();
let shared = arena.alloc_arc(42_u64);
let h = std::thread::spawn(move || *shared);
assert_eq!(42, h.join().unwrap());
```

[`Box`][__link24] is a unique owner whose `Drop` runs `T::drop` immediately
when the smart pointer is dropped and provides `&mut T` access, similar to
[`alloc::boxed::Box`][__link25] but backed by the arena.

```rust
let arena = multitude::Arena::new();
let mut v = arena.alloc_box(vec![1, 2, 3]);
v.push(4);
assert_eq!(*v, vec![1, 2, 3, 4]);
drop(v); // The vec drop runs here, freeing its heap buffer.
```

### Collections

[`Vec`][__link26] and [`String`][__link27] are growable collections that live in
the arena.

Additionally, you can use an arena as
the allocator for any type from the [`allocator-api2`][__link28] ecosystem
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

[`String`][__link29] and [`Vec`][__link30] are designed as **transient
builders**. They carry a data pointer + length + capacity + arena reference.

Once you’re done building, you can **freeze them** into immutable smart pointers:

* [`String::into_boxed_str`][__link31] →
  [`Box<str>`][__link32] (**8 bytes**, thin), or `Box::from(string)`.
  The freeze is **O(n)** — it copies the bytes into a compact,
  length-prefixed allocation so the resulting single pointer can outlive
  the arena. (Like any [`Box`][__link33], it is `Send`/`Sync` only when the
  allocator `A` is.)
* [`Vec::into_boxed_slice`][__link34] →
  [`Box<[T]>`][__link35] (**8 bytes**, thin), or `Box::from(vec)`.
  The freeze is **O(n)** — it moves the elements into a fresh compact,
  length-prefixed allocation so the resulting single pointer can outlive
  the arena. (Like any [`Box`][__link36], it is `Send`/`Sync` only when `T` and the
  allocator `A` are.)
* `Arc::from(vec)` / `Arc::from(string)` → [`Arc<[T]>`][__link37] /
  [`Arc<str>`][__link38], the shared, reference-counted freeze
  (mirroring `std`’s `From<Vec<T>> for Arc<[T]>`).
* [`Vec::leak`][__link39] → `&mut [T]` (or `&*v.leak()` for `&[T]`)
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

See [`BUMPALO.md`][__link40]
for a feature-by-feature comparison with [`bumpalo`][__link41].

## Strings

`multitude` provides a family of arena-resident string types in the
[`strings`][__link42] module. The model is the same one used for arbitrary
values elsewhere in the crate — bump-allocation backed by a per-chunk
refcount — but specialized for UTF-8 / UTF-16 text and a compact
single-pointer representation.

There are two roles a string type can play:

1. **Smart pointers (immutable / owned).** Compact handles to string
   data already stored in the arena. They use a single-pointer (8
   bytes on 64-bit) layout — half the size of `&str` — by storing
   the length unaligned in the chunk prefix. They differ in how
   sharing and mutability work:
   
   |UTF-8|UTF-16|Sharing|Mutable|Notes|
   |-----|------|-------|-------|-----|
   |[`Arc<str>`][__link43]|[`ArcUtf16Str`][__link44]|atomic refcount; `Clone`, `Send + Sync`|no|cross-thread sharing|
   |[`Box<str>`][__link45]|[`BoxUtf16Str`][__link46]|unique owner; `Send + Sync` (not `Clone`)|yes|drops eagerly|
   
   Like the other arena smart pointers, they keep their owning chunk
   alive via a refcount, so they can outlive the [`Arena`][__link47] they came
   from.

1. **Builders (mutable, growable).** [`String`][__link48] and
   [`Utf16String`][__link49] are transient growable
   buffers — small structs (32 bytes) carrying a data pointer +
   length + capacity + arena reference. You build them up with
   `push_str` / `push` / [`format!`][__link50] /
   [`format_utf16!`][__link51], then **freeze** them
   into one of the smart pointers above:
   
   |Builder|Freeze method|Result|
   |-------|-------------|------|
   |[`String`][__link52]|[`into_boxed_str`][__link53]|[`Box<str>`][__link54]|
   |[`Utf16String`][__link55]|[`into_boxed_utf16_str`][__link56]|[`BoxUtf16Str`][__link57]|
   
   The UTF-16 freeze reuses the buffer in place (O(1)) and returns
   any unused tail capacity to the chunk’s bump cursor when it can.
   The UTF-8 freeze copies the bytes (O(n)) into a compact,
   length-prefixed allocation so [`Box<str>`][__link58] stays a
   single, `Send`-safe pointer.

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

## Building DSTs

With the `dst` Cargo feature enabled, [`Arena`][__link59] exposes
[`Arena::alloc_dst_arc`][__link60] and
[`Arena::alloc_dst_box`][__link61] (and their `try_*` siblings) for
constructing values whose layout is only known at runtime (custom
DSTs, fat pointers, trait objects).

Each of these takes a [`Layout`][__link62], a
pointer-metadata value (e.g. a slice length, a `DynMetadata`), and
a closure that initializes the buffer through a typed fat pointer.
For most users, the [`dst-factory`][__link63] companion crate is the
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
|`std` *(default)*|Enables [`std::io::Write`][__link64] on [`Vec<u8>`][__link65] for use with `write!`, `std::io::copy`, `serde_json::to_writer`, and similar. Disable for `#![no_std]` environments (the crate still requires `alloc`).|
|`stats`|Enables runtime instrumentation counters returned by `Arena::stats`. Disable for the tightest allocation throughput when you don’t need observability.|
|`serde`|Adds `Serialize` impls for [`Arc<str>`][__link66], [`Box<str>`][__link67], [`String`][__link68], and [`Vec`][__link69]. With `serde + utf16`, also adds impls for the UTF-16 types (transcoded to UTF-8 on the wire).|
|`dst`|Enables the `dst` module for constructing true dynamically-sized types and trait objects in the arena via [`Arena::alloc_dst_arc`][__link70] / [`Arena::alloc_dst_box`][__link71], plus eight `Arena::alloc_slice_*_box` methods.|
|`utf16`|Adds a parallel UTF-16 string surface ([`ArcUtf16Str`][__link72], [`BoxUtf16Str`][__link73], [`Utf16String`][__link74], and [`format_utf16!`][__link75]) backed by the [`widestring`][__link76] crate. Lengths are counted in `u16` elements.|
|`zerocopy`|Provides [`ZerocopyView`][__link77] for safe zero-initialized allocation of types implementing [`zerocopy::FromZeros`][__link78]. Access via [`Arena::zerocopy()`][__link79].|
|`bytemuck`|Provides [`BytemuckView`][__link80] for safe zero-initialized allocation of types implementing [`bytemuck::Zeroable`][__link81]. Access via [`Arena::bytemuck()`][__link82].|
|`bytes`|Adds [`From`][__link83] conversions from [`Arc<[u8]>`][__link84] and [`Arc<str>`][__link85] into [`bytes::Bytes`][__link86], enabling zero-copy integration with the Tokio / Hyper async ecosystem.|
|`bytesbuf`|Implements [`bytesbuf::mem::Memory`][__link87] directly on [`Arena`][__link88], so that [`BytesBuf`][__link89] buffers can be backed by arena chunks. Implies `std`.|


<hr/>
<sub>
This crate was developed as part of <a href="../..">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/multitude">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjJhdIQbLiTyV0MU86EbZU15e0PmecoboQ9jo59bnAEbyDXw04U13GlhYvRhcoQbBzV3ofWgqIgbt8brW1MeN_Mb9N6Ac8XJFEIbIYjmnKUrOjRhZIWCaGJ5dGVtdWNrZjEuMjUuMIJlYnl0ZXNmMS4xMS4xgmhieXRlc2J1ZmUwLjUuNIJpbXVsdGl0dWRlZTAuMy4wgmh6ZXJvY29weWYwLjguNTA
 [__link0]: https://crates.io/crates/bumpalo
 [__link1]: https://docs.rs/multitude/0.3.0/multitude/?search=Arc
 [__link10]: https://docs.rs/multitude/0.3.0/multitude/?search=vec::Vec
 [__link11]: https://crates.io/crates/dst-factory
 [__link12]: https://docs.rs/multitude/0.3.0/multitude/?search=strings::format
 [__link13]: https://docs.rs/multitude/0.3.0/multitude/?search=strings::ArcUtf16Str
 [__link14]: https://docs.rs/multitude/0.3.0/multitude/?search=strings::BoxUtf16Str
 [__link15]: https://docs.rs/multitude/0.3.0/multitude/?search=strings::Utf16String
 [__link16]: https://docs.rs/multitude/0.3.0/multitude/?search=strings::format_utf16
 [__link17]: https://docs.rs/multitude/0.3.0/multitude/?search=Arc
 [__link18]: https://docs.rs/multitude/0.3.0/multitude/?search=Box
 [__link19]: https://docs.rs/multitude/0.3.0/multitude/?search=Arena
 [__link2]: https://docs.rs/multitude/0.3.0/multitude/?search=Arc
 [__link20]: https://doc.rust-lang.org/stable/std/marker/trait.Send.html
 [__link21]: https://docs.rs/multitude/0.3.0/multitude/?search=Arc
 [__link22]: https://docs.rs/multitude/0.3.0/multitude/?search=Arc
 [__link23]: https://docs.rs/multitude/0.3.0/multitude/?search=Arc
 [__link24]: https://docs.rs/multitude/0.3.0/multitude/?search=Box
 [__link25]: https://doc.rust-lang.org/stable/alloc/?search=boxed::Box
 [__link26]: https://docs.rs/multitude/0.3.0/multitude/?search=vec::Vec
 [__link27]: https://docs.rs/multitude/0.3.0/multitude/?search=strings::String
 [__link28]: https://crates.io/crates/allocator-api2
 [__link29]: https://docs.rs/multitude/0.3.0/multitude/?search=strings::String
 [__link3]: https://docs.rs/multitude/0.3.0/multitude/?search=Arc
 [__link30]: https://docs.rs/multitude/0.3.0/multitude/?search=vec::Vec
 [__link31]: https://docs.rs/multitude/0.3.0/multitude/?search=strings::String::into_boxed_str
 [__link32]: https://docs.rs/multitude/0.3.0/multitude/?search=Box
 [__link33]: https://docs.rs/multitude/0.3.0/multitude/?search=Box
 [__link34]: https://docs.rs/multitude/0.3.0/multitude/?search=vec::Vec::into_boxed_slice
 [__link35]: https://docs.rs/multitude/0.3.0/multitude/?search=Box
 [__link36]: https://docs.rs/multitude/0.3.0/multitude/?search=Box
 [__link37]: https://docs.rs/multitude/0.3.0/multitude/?search=Arc
 [__link38]: https://docs.rs/multitude/0.3.0/multitude/?search=Arc
 [__link39]: https://docs.rs/multitude/0.3.0/multitude/?search=vec::Vec::leak
 [__link4]: https://docs.rs/multitude/0.3.0/multitude/?search=Box
 [__link40]: https://github.com/microsoft/oxidizer/blob/main/crates/multitude/BUMPALO.md
 [__link41]: https://crates.io/crates/bumpalo
 [__link42]: https://docs.rs/multitude/0.3.0/multitude/strings/index.html
 [__link43]: https://docs.rs/multitude/0.3.0/multitude/?search=Arc
 [__link44]: https://docs.rs/multitude/0.3.0/multitude/?search=strings::ArcUtf16Str
 [__link45]: https://docs.rs/multitude/0.3.0/multitude/?search=Box
 [__link46]: https://docs.rs/multitude/0.3.0/multitude/?search=strings::BoxUtf16Str
 [__link47]: https://docs.rs/multitude/0.3.0/multitude/?search=Arena
 [__link48]: https://docs.rs/multitude/0.3.0/multitude/?search=strings::String
 [__link49]: https://docs.rs/multitude/0.3.0/multitude/?search=strings::Utf16String
 [__link5]: https://docs.rs/multitude/0.3.0/multitude/?search=Box
 [__link50]: https://docs.rs/multitude/0.3.0/multitude/?search=strings::format
 [__link51]: https://docs.rs/multitude/0.3.0/multitude/?search=strings::format_utf16
 [__link52]: https://docs.rs/multitude/0.3.0/multitude/?search=strings::String
 [__link53]: https://docs.rs/multitude/0.3.0/multitude/?search=strings::String::into_boxed_str
 [__link54]: https://docs.rs/multitude/0.3.0/multitude/?search=Box
 [__link55]: https://docs.rs/multitude/0.3.0/multitude/?search=strings::Utf16String
 [__link56]: https://docs.rs/multitude/0.3.0/multitude/?search=strings::Utf16String::into_boxed_utf16_str
 [__link57]: https://docs.rs/multitude/0.3.0/multitude/?search=strings::BoxUtf16Str
 [__link58]: https://docs.rs/multitude/0.3.0/multitude/?search=Box
 [__link59]: https://docs.rs/multitude/0.3.0/multitude/?search=Arena
 [__link6]: https://docs.rs/multitude/0.3.0/multitude/?search=Box
 [__link60]: https://docs.rs/multitude/0.3.0/multitude/?search=Arena::alloc_dst_arc
 [__link61]: https://docs.rs/multitude/0.3.0/multitude/?search=Arena::alloc_dst_box
 [__link62]: https://doc.rust-lang.org/stable/core/?search=alloc::Layout
 [__link63]: https://crates.io/crates/dst-factory
 [__link64]: https://doc.rust-lang.org/stable/std/?search=io::Write
 [__link65]: https://docs.rs/multitude/0.3.0/multitude/?search=vec::Vec
 [__link66]: https://docs.rs/multitude/0.3.0/multitude/?search=Arc
 [__link67]: https://docs.rs/multitude/0.3.0/multitude/?search=Box
 [__link68]: https://docs.rs/multitude/0.3.0/multitude/?search=strings::String
 [__link69]: https://docs.rs/multitude/0.3.0/multitude/?search=vec::Vec
 [__link7]: https://docs.rs/multitude/0.3.0/multitude/?search=Arc
 [__link70]: https://docs.rs/multitude/0.3.0/multitude/?search=Arena::alloc_dst_arc
 [__link71]: https://docs.rs/multitude/0.3.0/multitude/?search=Arena::alloc_dst_box
 [__link72]: https://docs.rs/multitude/0.3.0/multitude/?search=strings::ArcUtf16Str
 [__link73]: https://docs.rs/multitude/0.3.0/multitude/?search=strings::BoxUtf16Str
 [__link74]: https://docs.rs/multitude/0.3.0/multitude/?search=strings::Utf16String
 [__link75]: https://docs.rs/multitude/0.3.0/multitude/?search=strings::format_utf16
 [__link76]: https://crates.io/crates/widestring
 [__link77]: https://docs.rs/multitude/0.3.0/multitude/?search=zerocopy::ZerocopyView
 [__link78]: https://docs.rs/zerocopy/0.8.50/zerocopy/?search=FromZeros
 [__link79]: https://docs.rs/multitude/0.3.0/multitude/?search=Arena::zerocopy
 [__link8]: https://docs.rs/multitude/0.3.0/multitude/?search=Box
 [__link80]: https://docs.rs/multitude/0.3.0/multitude/?search=bytemuck::BytemuckView
 [__link81]: https://docs.rs/bytemuck/1.25.0/bytemuck/?search=Zeroable
 [__link82]: https://docs.rs/multitude/0.3.0/multitude/?search=Arena::bytemuck
 [__link83]: https://doc.rust-lang.org/stable/std/convert/trait.From.html
 [__link84]: https://docs.rs/multitude/0.3.0/multitude/?search=Arc
 [__link85]: https://docs.rs/multitude/0.3.0/multitude/?search=Arc
 [__link86]: https://docs.rs/bytes/1.11.1/bytes/?search=Bytes
 [__link87]: https://docs.rs/bytesbuf/0.5.4/bytesbuf/?search=mem::Memory
 [__link88]: https://docs.rs/multitude/0.3.0/multitude/?search=Arena
 [__link89]: https://docs.rs/bytesbuf/0.5.4/bytesbuf/?search=BytesBuf
 [__link9]: https://docs.rs/multitude/0.3.0/multitude/?search=strings::String
