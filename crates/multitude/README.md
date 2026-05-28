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
   * Reference-counted smart pointers ([`Rc`][__link1], [`RcStr`][__link2]) for
     single-threaded sharing.
   * Atomic reference-counted smart pointers ([`Arc`][__link3], [`ArcStr`][__link4])
     for cross-thread sharing.
   * Owned, mutable smart pointers ([`Box`][__link5], [`BoxStr`][__link6]).
1. **Early Reclamation.** In many situations, `multitude` can reclaim memory from individual chunks as soon as their reference counts drop to zero,
   without waiting for the entire arena to be dropped. This allows for more efficient memory usage in long-running arenas with many short-lived allocations.

1. **Smart Pointers Can Outlive the Arena.** The reference-counted smart pointers produced by `multitude` can keep their owning chunk alive even after the arena itself has been dropped,
   allowing for more flexible memory management and longer-lived data structures.

1. **Drop Support.** `multitude` automatically runs `Drop` for allocated values at the appropriate time.

1. **Efficient Immutable String References.** `multitude` provides [`RcStr`][__link7], [`ArcStr`][__link8], and
   [`BoxStr`][__link9] — single-pointer (8 bytes) smart pointers to UTF-8 strings stored in the arena. Refcounted, atomic-refcounted,
   and owned-mutable variants respectively, all sharing the same compact layout.

1. **Efficient Mutable Strings and Vectors.** `multitude` provides [`String`][__link10] and [`Vec`][__link11] which are growable collections that live in the arena and can be frozen into compact
   reference-counted smart pointers when you’re done building.

1. **Dynamically-Sized Types.** `multitude` supports dynamically-sized types (DSTs) like slices and strings, allowing you to allocate and manage them in the
   arena with the same flexibility as sized types. The [`dst-factory`][__link12] crate is a great companion for building DSTs in the arena.

1. **`format!`-style Macro.** `multitude` includes a [`format!`][__link13]-style macro that allows you to create formatted strings directly in the arena, avoiding intermediate allocations and copies.

1. **UTF-16 Support.** With the `utf16` Cargo feature, `multitude` provides a parallel set of arena-resident UTF-16 string types ([`RcUtf16Str`][__link14],
   [`ArcUtf16Str`][__link15], [`BoxUtf16Str`][__link16], [`Utf16String`][__link17]) and a [`format_utf16!`][__link18] macro for FFI / Windows / JS-engine
   interop without per-call transcoding at every boundary.

1. **`#![no_std]` Support.** `multitude` can be used in `#![no_std]` environments, making it suitable for embedded systems and other resource-constrained contexts.

## Example

```rust
use multitude::Arena;

let arena = Arena::new();

// Cheap reference-counted allocation of any user type.
struct Point { x: f64, y: f64 }
let p = arena.alloc_rc(Point { x: 3.0, y: 4.0 });
let p2 = p.clone();
assert_eq!(p.x, p2.x);

// Single-pointer immutable strings.
let name = arena.alloc_str_rc("Alice");
assert_eq!(&*name, "Alice");

// format! macro returning a String (call .into_arena_str() to
// freeze into a compact 8-byte RcStr).
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

Smart pointers ([`Rc`][__link19], [`Arc`][__link20], [`Box`][__link21] and their `str` variations) work in a way similar to the like-named types
in the standard library, except that they reference addresses within an arena.

```rust
use multitude::Rc;

struct Point {
    x: f64,
    y: f64,
}

let p: Rc<Point> = {
    let arena = multitude::Arena::new();
    arena.alloc_rc(Point { x: 3.0, y: 4.0 })
    // arena dropped here
};
assert_eq!(p.x, 3.0);
```

Although [`Arena`][__link22] itself is single-threaded (`!Send` and `!Sync`), the arc-family of types (e.g., [`Arc`][__link23]) enable cross-thread sharing.

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

* [`String::into_arena_str`][__link31] → [`RcStr`][__link32] (**8 bytes**). The
  freeze itself is **O(1)** — no copy, no new allocation.
* [`Vec::into_arena_rc`][__link33] → [`Rc<[T]>`][__link34] (16-byte slice fat
  pointer; immutable, cloneable, refcount-based). For `T: !Drop`,
  the freeze is **O(1)** too.

Both freezes also reclaim any unused capacity left in the buffer
when the conditions allow it, so those bytes become available for
the next allocation.

```rust
use multitude::Arena;
use multitude::strings::RcStr;

let arena = Arena::new();

// Build phase: 32-byte builder, alive briefly.
let mut builder = arena.alloc_string();
builder.push_str("hello, ");
builder.push_str("world");

// Freeze for storage: 8-byte single-pointer smart pointer. O(1) — no copy.
let stored: RcStr = builder.into_arena_str();
assert_eq!(&*stored, "hello, world");
```

Use this pattern whenever you’d be storing many strings or slices
long-term — the per-pointer savings (16 bytes for strings, 8 for
slices) add up quickly across millions of items, and the frozen
smart pointers are also cheaper to clone.

See [`BUMPALO.md`][__link35]
for a feature-by-feature comparison with [`bumpalo`][__link36].

## Strings

`multitude` provides a family of arena-resident string types in the
[`strings`][__link37] module. The model is the same one used for arbitrary
values elsewhere in the crate — bump-allocation backed by a per-chunk
refcount — but specialized for UTF-8 / UTF-16 text and a compact
single-pointer representation.

There are two roles a string type can play:

1. **Smart pointers (immutable / owned).** Compact handles to string
   data already stored in the arena. They use a single-pointer (8
   bytes on 64-bit) layout — half the size of `&str` — by storing
   the length inline with the string bytes. They differ in how
   sharing and mutability work:
   
   |UTF-8|UTF-16|Sharing|Mutable|Notes|
   |-----|------|-------|-------|-----|
   |[`RcStr`][__link38]|[`RcUtf16Str`][__link39]|refcount, `!Send`/`!Sync`|no|cheapest clone|
   |[`ArcStr`][__link40]|[`ArcUtf16Str`][__link41]|atomic refcount, `Send + Sync`|no|cross-thread|
   |[`BoxStr`][__link42]|[`BoxUtf16Str`][__link43]|unique owner|yes|drops eagerly|
   
   Like the other arena smart pointers, they keep their owning chunk
   alive via a refcount, so they can outlive the [`Arena`][__link44] they came
   from.

1. **Builders (mutable, growable).** [`String`][__link45] and
   [`Utf16String`][__link46] are transient growable
   buffers — small structs (32 bytes) carrying a data pointer +
   length + capacity + arena reference. You build them up with
   `push_str` / `push_char` / [`format!`][__link47] /
   [`format_utf16!`][__link48], then **freeze** them
   in O(1) into one of the smart pointers above:
   
   |Builder|Freeze method|Result|
   |-------|-------------|------|
   |[`String`][__link49]|[`into_arena_str`][__link50]|[`RcStr`][__link51]|
   |[`Utf16String`][__link52]|[`into_arena_utf16_str`][__link53]|[`RcUtf16Str`][__link54]|
   
   The freeze reuses the buffer in place — no copy — and returns
   any unused tail capacity to the chunk’s bump cursor when it can.

UTF-16 support requires the `utf16` Cargo feature. Strict (validated)
UTF-16 only — lone surrogates are rejected. The UTF-16 types
interoperate with `widestring::Utf16Str` / `widestring::Utf16String`
for I/O and FFI bridging. UTF-16 length and capacity are counted in
`u16` elements (matching `widestring::Utf16Str::len()`).

### Example: UTF-8

```rust
use multitude::Arena;
use multitude::strings::RcStr;

let arena = Arena::new();

// Single-pointer immutable strings.
let s = arena.alloc_str_rc("hello, world");
assert_eq!(&*s, "hello, world");

// Build incrementally and freeze:
let mut b = arena.alloc_string();
b.push_str("abc");
b.push_str("123");
let frozen: RcStr = b.into_arena_str();
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
let s = arena.alloc_utf16_str_rc(utf16str!("hello, world"));
assert_eq!(&*s, utf16str!("hello, world"));

// Or transcode from a &str:
let s2 = arena.alloc_utf16_str_rc_from_str("hello");
assert_eq!(&*s2, utf16str!("hello"));

// Build incrementally and freeze:
let mut b = arena.alloc_utf16_string();
b.push_str(utf16str!("abc"));
b.push_from_str("123");
let frozen = b.into_arena_utf16_str();
assert_eq!(&*frozen, utf16str!("abc123"));

// format!-style:
let name = "Alice";
let greeting = multitude::strings::format_utf16!(in &arena, "Hello, {name}!");
assert_eq!(greeting.as_utf16_str(), utf16str!("Hello, Alice!"));
```

## Building DSTs

With the `dst` Cargo feature enabled, [`Arena`][__link55] exposes
[`Arena::alloc_dst_arc`][__link56] / [`Arena::alloc_dst_rc`][__link57] /
[`Arena::alloc_dst_box`][__link58] (and their `try_*` siblings) for
constructing values whose layout is only known at runtime (custom
DSTs, fat pointers, trait objects).

Each of these takes a [`Layout`][__link59], a
pointer-metadata value (e.g. a slice length, a `DynMetadata`), and
a closure that initializes the buffer through a typed fat pointer.
For most users, the [`dst-factory`][__link60] companion crate is the
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
existing `_rc`/`_arc` slice methods).

## Crate Features

|Feature|Description|
|-------|-----------|
|`std` *(default)*|Enables [`std::io::Write`][__link61] on [`Vec<u8>`][__link62] for use with `write!`, `std::io::copy`, `serde_json::to_writer`, and similar. Disable for `#![no_std]` environments (the crate still requires `alloc`).|
|`stats`|Enables runtime instrumentation counters returned by `Arena::stats`. Disable for the tightest allocation throughput when you don’t need observability.|
|`serde`|Adds `Serialize` impls for [`RcStr`][__link63], [`ArcStr`][__link64], [`String`][__link65], and [`Vec`][__link66]. With `serde + utf16`, also adds impls for the UTF-16 types (transcoded to UTF-8 on the wire).|
|`dst`|Enables the `dst` module for constructing true dynamically-sized types and trait objects in the arena via [`Arena::alloc_dst_arc`][__link67] / [`Arena::alloc_dst_rc`][__link68] / [`Arena::alloc_dst_box`][__link69], plus eight `Arena::alloc_slice_*_box` methods.|
|`utf16`|Adds a parallel UTF-16 string surface ([`RcUtf16Str`][__link70], [`ArcUtf16Str`][__link71], [`BoxUtf16Str`][__link72], [`Utf16String`][__link73], and [`format_utf16!`][__link74]) backed by the [`widestring`][__link75] crate. Lengths are counted in `u16` elements.|
|`zerocopy`|Provides [`ZerocopyView`][__link76] for safe zero-initialized allocation of types implementing [`zerocopy::FromZeros`][__link77]. Access via [`Arena::zerocopy()`][__link78].|
|`bytemuck`|Provides [`BytemuckView`][__link79] for safe zero-initialized allocation of types implementing [`bytemuck::Zeroable`][__link80]. Access via [`Arena::bytemuck()`][__link81].|
|`bytes`|Adds [`From`][__link82] conversions from [`Arc<[u8]>`][__link83] and [`ArcStr`][__link84] into [`bytes::Bytes`][__link85], enabling zero-copy integration with the Tokio / Hyper async ecosystem.|
|`bytesbuf`|Implements [`bytesbuf::mem::Memory`][__link86] directly on [`Arena`][__link87], so that [`BytesBuf`][__link88] buffers can be backed by arena chunks. Implies `std`.|


<hr/>
<sub>
This crate was developed as part of <a href="../..">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/multitude">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGkYW0CYXSEGy4k8ldDFPOhG2VNeXtD5nnKG6EPY6OfW5wBG8g18NOFNdxpYXKEGxSsckQO6X9rG16hze1SNSzMGw6W-TYGhj3rG1Gc1jfyNoYcYWSFgmhieXRlbXVja2YxLjI1LjCCZWJ5dGVzZjEuMTEuMYJoYnl0ZXNidWZlMC41LjCCaW11bHRpdHVkZWUwLjEuMIJoemVyb2NvcHlmMC44LjQ5
 [__link0]: https://crates.io/crates/bumpalo
 [__link1]: https://docs.rs/multitude/0.1.0/multitude/?search=rc::Rc
 [__link10]: https://docs.rs/multitude/0.1.0/multitude/?search=strings::String
 [__link11]: https://docs.rs/multitude/0.1.0/multitude/?search=vec::Vec
 [__link12]: https://crates.io/crates/dst-factory
 [__link13]: strings::format!
 [__link14]: https://docs.rs/multitude/0.1.0/multitude/?search=strings::RcUtf16Str
 [__link15]: https://docs.rs/multitude/0.1.0/multitude/?search=strings::ArcUtf16Str
 [__link16]: https://docs.rs/multitude/0.1.0/multitude/?search=strings::BoxUtf16Str
 [__link17]: https://docs.rs/multitude/0.1.0/multitude/?search=strings::Utf16String
 [__link18]: strings::format_utf16!
 [__link19]: https://docs.rs/multitude/0.1.0/multitude/?search=rc::Rc
 [__link2]: https://docs.rs/multitude/0.1.0/multitude/?search=strings::RcStr
 [__link20]: https://docs.rs/multitude/0.1.0/multitude/?search=arc::Arc
 [__link21]: https://docs.rs/multitude/0.1.0/multitude/?search=r#box::Box
 [__link22]: https://docs.rs/multitude/0.1.0/multitude/?search=arena::Arena
 [__link23]: https://docs.rs/multitude/0.1.0/multitude/?search=arc::Arc
 [__link24]: https://docs.rs/multitude/0.1.0/multitude/?search=r#box::Box
 [__link25]: https://doc.rust-lang.org/stable/alloc/?search=boxed::Box
 [__link26]: https://docs.rs/multitude/0.1.0/multitude/?search=vec::Vec
 [__link27]: https://docs.rs/multitude/0.1.0/multitude/?search=strings::String
 [__link28]: https://crates.io/crates/allocator-api2
 [__link29]: https://docs.rs/multitude/0.1.0/multitude/?search=strings::String
 [__link3]: https://docs.rs/multitude/0.1.0/multitude/?search=arc::Arc
 [__link30]: https://docs.rs/multitude/0.1.0/multitude/?search=vec::Vec
 [__link31]: https://docs.rs/multitude/0.1.0/multitude/?search=strings::String::into_arena_str
 [__link32]: https://docs.rs/multitude/0.1.0/multitude/?search=strings::RcStr
 [__link33]: https://docs.rs/multitude/0.1.0/multitude/?search=vec::Vec::into_arena_rc
 [__link34]: https://docs.rs/multitude/0.1.0/multitude/?search=Rc
 [__link35]: https://github.com/microsoft/oxidizer/blob/main/crates/multitude/BUMPALO.md
 [__link36]: https://crates.io/crates/bumpalo
 [__link37]: https://docs.rs/multitude/0.1.0/multitude/strings/index.html
 [__link38]: https://docs.rs/multitude/0.1.0/multitude/?search=strings::RcStr
 [__link39]: https://docs.rs/multitude/0.1.0/multitude/?search=strings::RcUtf16Str
 [__link4]: https://docs.rs/multitude/0.1.0/multitude/?search=strings::ArcStr
 [__link40]: https://docs.rs/multitude/0.1.0/multitude/?search=strings::ArcStr
 [__link41]: https://docs.rs/multitude/0.1.0/multitude/?search=strings::ArcUtf16Str
 [__link42]: https://docs.rs/multitude/0.1.0/multitude/?search=strings::BoxStr
 [__link43]: https://docs.rs/multitude/0.1.0/multitude/?search=strings::BoxUtf16Str
 [__link44]: https://docs.rs/multitude/0.1.0/multitude/?search=arena::Arena
 [__link45]: https://docs.rs/multitude/0.1.0/multitude/?search=strings::String
 [__link46]: https://docs.rs/multitude/0.1.0/multitude/?search=strings::Utf16String
 [__link47]: strings::format!
 [__link48]: strings::format_utf16!
 [__link49]: https://docs.rs/multitude/0.1.0/multitude/?search=strings::String
 [__link5]: https://docs.rs/multitude/0.1.0/multitude/?search=r#box::Box
 [__link50]: https://docs.rs/multitude/0.1.0/multitude/?search=strings::String::into_arena_str
 [__link51]: https://docs.rs/multitude/0.1.0/multitude/?search=strings::RcStr
 [__link52]: https://docs.rs/multitude/0.1.0/multitude/?search=strings::Utf16String
 [__link53]: https://docs.rs/multitude/0.1.0/multitude/?search=strings::Utf16String::into_arena_utf16_str
 [__link54]: https://docs.rs/multitude/0.1.0/multitude/?search=strings::RcUtf16Str
 [__link55]: https://docs.rs/multitude/0.1.0/multitude/?search=arena::Arena
 [__link56]: https://docs.rs/multitude/0.1.0/multitude/?search=arena::Arena::alloc_dst_arc
 [__link57]: https://docs.rs/multitude/0.1.0/multitude/?search=arena::Arena::alloc_dst_rc
 [__link58]: https://docs.rs/multitude/0.1.0/multitude/?search=arena::Arena::alloc_dst_box
 [__link59]: https://doc.rust-lang.org/stable/core/?search=alloc::Layout
 [__link6]: https://docs.rs/multitude/0.1.0/multitude/?search=strings::BoxStr
 [__link60]: https://crates.io/crates/dst-factory
 [__link61]: https://doc.rust-lang.org/stable/std/?search=io::Write
 [__link62]: https://docs.rs/multitude/0.1.0/multitude/?search=vec::Vec
 [__link63]: https://docs.rs/multitude/0.1.0/multitude/?search=strings::RcStr
 [__link64]: https://docs.rs/multitude/0.1.0/multitude/?search=strings::ArcStr
 [__link65]: https://docs.rs/multitude/0.1.0/multitude/?search=strings::String
 [__link66]: https://docs.rs/multitude/0.1.0/multitude/?search=vec::Vec
 [__link67]: https://docs.rs/multitude/0.1.0/multitude/?search=arena::Arena::alloc_dst_arc
 [__link68]: https://docs.rs/multitude/0.1.0/multitude/?search=arena::Arena::alloc_dst_rc
 [__link69]: https://docs.rs/multitude/0.1.0/multitude/?search=arena::Arena::alloc_dst_box
 [__link7]: https://docs.rs/multitude/0.1.0/multitude/?search=strings::RcStr
 [__link70]: https://docs.rs/multitude/0.1.0/multitude/?search=strings::RcUtf16Str
 [__link71]: https://docs.rs/multitude/0.1.0/multitude/?search=strings::ArcUtf16Str
 [__link72]: https://docs.rs/multitude/0.1.0/multitude/?search=strings::BoxUtf16Str
 [__link73]: https://docs.rs/multitude/0.1.0/multitude/?search=strings::Utf16String
 [__link74]: strings::format_utf16!
 [__link75]: https://crates.io/crates/widestring
 [__link76]: https://docs.rs/multitude/0.1.0/multitude/?search=zerocopy::ZerocopyView
 [__link77]: https://docs.rs/zerocopy/0.8.49/zerocopy/?search=FromZeros
 [__link78]: `Arena::zerocopy()`
 [__link79]: https://docs.rs/multitude/0.1.0/multitude/?search=bytemuck::BytemuckView
 [__link8]: https://docs.rs/multitude/0.1.0/multitude/?search=strings::ArcStr
 [__link80]: https://docs.rs/bytemuck/1.25.0/bytemuck/?search=Zeroable
 [__link81]: `Arena::bytemuck()`
 [__link82]: https://doc.rust-lang.org/stable/std/convert/trait.From.html
 [__link83]: https://docs.rs/multitude/0.1.0/multitude/?search=arc::Arc
 [__link84]: https://docs.rs/multitude/0.1.0/multitude/?search=strings::ArcStr
 [__link85]: https://docs.rs/bytes/1.11.1/bytes/?search=Bytes
 [__link86]: https://docs.rs/bytesbuf/0.5.0/bytesbuf/?search=mem::Memory
 [__link87]: https://docs.rs/multitude/0.1.0/multitude/?search=arena::Arena
 [__link88]: https://docs.rs/bytesbuf/0.5.0/bytesbuf/?search=BytesBuf
 [__link9]: https://docs.rs/multitude/0.1.0/multitude/?search=strings::BoxStr
