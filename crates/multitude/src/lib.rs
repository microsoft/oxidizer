// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![allow(
    clippy::multiple_unsafe_ops_per_block,
    clippy::allow_attributes,
    reason = "throughout this crate, related unsafe operations are intentionally grouped under a single safety invariant; `#[allow]` is preferred over `#[expect]` for attributes that expand inside macro bodies (the lint may or may not fire in any given instantiation)"
)]

//! Fast and flexible arena-based bump allocator.
//!
//! `multitude` is a bump allocator for applications with phase-oriented lifetimes.
//!
//! Groups of related allocations live and die together. Service request handling and parsers are two examples of this pattern which usually
//! benefit from a bump allocator.
//!
//! `multitude` works by accumulating large chunks of memory allocated from the system and then carving out smaller pieces of it for application use
//! using a fast bump allocation strategy, which is considerably faster than allocating from the system. The downside however is that the individual allocations
//! can't be freed separately. Instead, memory is reclaimed and returned to the system in bulk when the entire arena is dropped.
//!
//! # Why Another Bump Allocator?
//!
//! The Rust ecosystem has a few bump allocators, the most popular being [`bumpalo`](https://crates.io/crates/bumpalo).
//! `multitude` uses a different implementation strategy and has a richer API surface making it suitable for more
//! use cases. The main features that set `multitude` apart are:
//!
//! 1. **Flexibility.** Four allocation styles coexist in the same arena: the
//!    arena-lifetime owning handle [`Alloc<T>`](Alloc) plus three escape-capable
//!    smart pointers — the atomic [`Arc`], the non-atomic single-thread [`Rc`],
//!    and the unique-owner [`Box`] — each available for sized `T`, `str`, and
//!    `[T]`. See the [comparison table](#flexibility) for how they differ.
//!
//! 2. **Early Reclamation.** In many situations, `multitude` can reclaim memory from individual chunks as soon as their reference counts drop to zero,
//!    without waiting for the entire arena to be dropped. This allows for more efficient memory usage in long-running arenas with many short-lived allocations.
//!
//! 3. **Smart Pointers Can Outlive the Arena.** Some of the smart pointers produced by `multitude` can keep their owning chunk alive even after the arena itself has been dropped,
//!    allowing for more flexible memory management and longer-lived data structures.
//!
//! 4. **Drop Support.** `multitude` automatically runs `Drop` for allocated values at the appropriate time.
//!
//! 5. **Uniformly Thin Smart Pointers.** `multitude`'s escape-capable smart
//!    pointers — [`Arc<T>`](Arc), [`Rc<T>`](Rc), and [`Box<T>`](Box) — are
//!    **8 bytes** on 64-bit for *every* `T`, even DSTs like `str` and `[T]`
//!    (the metadata lives in a chunk prefix). The arena-lifetime
//!    [`Alloc<T>`](Alloc) handle is a single word for sized `T`; for `str` /
//!    `[T]` it is a fat reference (pointer + length), which costs nothing extra
//!    since it never escapes the arena and isn't stored at scale.
//!
//! 6. **Efficient Mutable Strings and Vectors.** `multitude` provides [`String`](strings::String), [`Utf16String`](strings::Utf16String) and [`Vec`](vec::Vec) which are growable collections that live in the arena.
//!
//! 7. **Dynamically-Sized Types.** `multitude` supports dynamically-sized types (DSTs) like slices and strings, allowing you to allocate and manage them in the
//!    arena with the same flexibility as sized types. The [`dst-factory`](https://crates.io/crates/dst-factory) crate is a great companion for building DSTs in the arena.
//!
//! 8. **First Class `serde` Support.**. `multitude` lets you deserialize data directly into
//!    arena-backed memory.
//!
//! 9. **`format!`-style Macro.** `multitude` includes a [`format!`](strings::format!)-style macro that allows you to create formatted strings directly in the arena, avoiding intermediate allocations and copies.
//!
//! 10. **UTF-16 Support.** With the `utf16` Cargo feature, `multitude` provides a parallel set of arena-resident UTF-16 string types
//!     (`Arc<Utf16Str>`, `Box<Utf16Str>`, [`Utf16String`](strings::Utf16String)) and a [`format_utf16!`](strings::format_utf16!) macro for FFI / Windows / JS-engine
//!     interop without per-call transcoding at every boundary.
//!
//! 11. **`#![no_std]` Support.** `multitude` can be used in `#![no_std]` environments, making it suitable for embedded systems and other resource-constrained contexts.
//!
//! See [`BUMPALO.md`](https://github.com/microsoft/oxidizer/blob/main/crates/multitude/docs/BUMPALO.md)
//! for a feature-by-feature comparison with [`bumpalo`](https://crates.io/crates/bumpalo).
//!
//! # Example
//!
//! ```
//! use multitude::Arena;
//!
//! let arena = Arena::new();
//!
//! // Cheap atomic reference-counted allocation of any user type.
//! struct Point { x: f64, y: f64 }
//! let p = arena.alloc_arc(Point { x: 3.0, y: 4.0 });
//! let p2 = p.clone();
//! assert_eq!(p.x, p2.x);
//!
//! // Single-pointer immutable strings.
//! let name = arena.alloc_str_arc("Alice");
//! assert_eq!(&*name, "Alice");
//!
//! // format! macro returning a String.
//! let greeting = multitude::strings::format!(in &arena, "Hello, {}!", "world");
//! assert_eq!(&*greeting, "Hello, world!");
//! ```
//!
//! # Flexibility
//!
//! `multitude` offers four ways to allocate a value and own it over time. All
//! four can coexist in the same arena, dereference to the value, and run
//! `T::drop` **eagerly**; they differ in whether the handle can outlive the
//! arena, whether ownership is unique or shared, and what (if any) per-handle
//! reference count they pay. Each is available for sized `T`, `str`, and `[T]`
//! (and, behind the `dst` feature, arbitrary DSTs).
//!
//! | | [`Alloc<T>`](Alloc) | [`Box<T>`](Box) | [`Rc<T>`](Rc) | [`Arc<T>`](Arc) |
//! |---|:---:|:---:|:---:|:---:|
//! | **Constructor family** | [`alloc`](Arena::alloc) | [`alloc_box`](Arena::alloc_box) | [`alloc_rc`](Arena::alloc_rc) | [`alloc_arc`](Arena::alloc_arc) |
//! | **Ownership** | unique | unique | shared (`Clone`) | shared (`Clone`) |
//! | **`&mut T` access** | ✅ | ✅ | ❌ | ❌ |
//! | **Can outlive the arena** | ❌ | ✅ | ✅ | ✅ |
//! | **Per-handle reference count** | none | none | non-atomic | atomic |
//! | **Cross-thread *sharing*** | ❌ | ❌ | ❌ (`!Send`) | ✅ (`T: Send + Sync`) |
//! | **Width (64-bit)** | 1 word (sized); fat ref for DSTs | 8 bytes | 8 bytes | 8 bytes |
//!
//! The cheapest option is [`Alloc<T>`](Alloc): an owning handle whose lifetime
//! is tied to the arena — a single word for sized `T` (a fat pointer+length
//! reference for `str` / `[T]`). It pays no reference count and cannot
//! outlive the arena, but gives mutable access and runs the destructor when it
//! is dropped — the fastest way to allocate when the lifetime constraint is
//! tolerable.
//!
//! ```
//! let arena = multitude::Arena::new();
//! let mut x = arena.alloc(42);
//! *x += 1;
//! assert_eq!(*x, 43);
//!
//! // Strings and slices too:
//! let s = arena.alloc_str("hello");
//! let v = arena.alloc_slice_copy(&[1, 2, 3]);
//! assert_eq!(&*s, "hello");
//! assert_eq!(&*v, &[1, 2, 3]);
//! ```
//!
//! For values that must **outlive the arena**, use one of the three smart
//! pointers. They behave like the like-named `std` types but are uniformly
//! **8-byte thin pointers** (even for DSTs) addressing storage inside a chunk,
//! and they keep that chunk alive until the last handle drops.
//!
//! [`Arc`] is reference-counted and shareable across threads:
//!
//! ```
//! use multitude::Arc;
//!
//! let p: Arc<u32> = {
//!     let arena = multitude::Arena::new();
//!     arena.alloc_arc(42)
//!     // arena dropped here; `p` keeps its chunk alive
//! };
//! assert_eq!(*p, 42);
//!
//! let arena = multitude::Arena::new();
//! let shared = arena.alloc_arc(7_u64);
//! let h = std::thread::spawn(move || *shared);
//! assert_eq!(7, h.join().unwrap());
//! ```
//!
//! [`Rc`] is the cheaper single-thread sibling of [`Arc`]: its reference count
//! is non-atomic, so `clone`/`drop` are cheaper and `str` / `[u8]` pack slightly
//! tighter. Being [`!Send`](Send)/[`!Sync`](Sync), it places **no** `Send`/`Sync`
//! bound on `T`, so it can share thread-affine values (e.g. `Rc<RefCell<T>>`)
//! that [`Arc`] cannot.
//!
//! ```
//! use multitude::Rc;
//!
//! let arena = multitude::Arena::new();
//! let a: Rc<u64> = arena.alloc_rc(42);
//! let b = a.clone();
//! assert_eq!(*a, *b);
//! ```
//!
//! [`Box`] is a unique owner that provides `&mut T` access, like
//! [`alloc::boxed::Box`] but backed by the arena:
//!
//! ```
//! let arena = multitude::Arena::new();
//! let mut v = arena.alloc_box(vec![1, 2, 3]);
//! v.push(4);
//! assert_eq!(*v, vec![1, 2, 3, 4]);
//! drop(v); // The vec drop runs here, freeing its heap buffer.
//! ```
//!
//! Although [`Arena`] itself is `!Sync`, it is [`Send`]: an arena — along with
//! any in-flight [`Alloc`] handles and smart pointers — can be moved between
//! threads. For cross-thread *sharing* of an individual value, allocate an
//! [`Arc`] and `.clone()` it across threads.
//!
//! # Collections
//!
//! [`Vec`](vec::Vec), [`String`](strings::String), and [`Utf16String`](strings::Utf16String) are growable collections that live in
//! the arena.
//!
//! Additionally, you can use an arena as
//! the allocator for any type from the [`allocator-api2`](https://crates.io/crates/allocator-api2) ecosystem
//! (including `hashbrown::HashMap`).
//!
//! ```
//! use multitude::Arena;
//! use multitude::vec::{CollectIn, Vec};
//!
//! let arena = Arena::new();
//!
//! let mut v = arena.alloc_vec::<i32>();
//! for i in 0..5 {
//!     v.push(i);
//! }
//!
//! // CollectIn trait for iterator collection.
//! let squares: Vec<i32, _> = (1..=5).map(|i| i * i).collect_in(&arena);
//! assert_eq!(squares.as_slice(), &[1, 4, 9, 16, 25]);
//! ```
//!
//! ## Freezing
//!
//! [`String`](strings::String) and [`Vec`](vec::Vec) are designed as **transient
//! builders** — mutable, growable handles meant to be used briefly and then frozen.
//!
//! Once you're done building, you can **freeze them** into immutable smart pointers:
//!
//! - [`String::into_boxed_str`](strings::String::into_boxed_str) →
//!   [`Box<str>`](crate::Box) (**8 bytes**, thin), or `Box::from(string)`.
//!   The freeze is **O(n)** — it copies the bytes into a compact allocation
//!   that can outlive the arena. (Like any [`Box`], it is `Send`/`Sync` only
//!   when the allocator `A` is.)
//! - [`Vec::into_boxed_slice`](vec::Vec::into_boxed_slice) →
//!   [`Box<[T]>`](crate::Box) (**8 bytes**, thin), or `Box::from(vec)`.
//!   The freeze is **O(n)** — it moves the elements into a fresh compact
//!   allocation that can outlive the arena. (Like any [`Box`], it is
//!   `Send`/`Sync` only when `T` and the allocator `A` are.)
//! - `Arc::from(vec)` / `Arc::from(string)` → [`Arc<[T]>`](crate::Arc) /
//!   [`Arc<str>`](crate::Arc), the shared, reference-counted freeze
//!   (mirroring `std`'s `From<Vec<T>> for Arc<[T]>`).
//! - [`Vec::leak`](vec::Vec::leak) → `&mut [T]` (or `&*v.leak()` for `&[T]`)
//!   borrowed for the arena's lifetime. For `T: !Drop`, this freeze is
//!   **O(1) and allocation-free** — the existing buffer is reinterpreted in
//!   place. Unlike the `Box`/`Arc` freezes, the slice does not outlive the arena.
//!
//! The `Vec` freeze also reclaims any unused capacity left in the
//! buffer when the conditions allow it, so those bytes become available
//! for the next allocation.
//!
//! ```
//! use multitude::{Arena, Box};
//!
//! let arena = Arena::new();
//!
//! // Build phase: 32-byte builder, alive briefly.
//! let mut builder = arena.alloc_string();
//! builder.push_str("hello, ");
//! builder.push_str("world");
//!
//! // Freeze for storage: 8-byte single-pointer smart pointer. O(n) — copies the bytes.
//! let stored: Box<str> = builder.into_boxed_str();
//! assert_eq!(&*stored, "hello, world");
//! ```
//!
//! Use this pattern whenever you'd be storing many strings or slices
//! long-term — the per-pointer savings (8 bytes for both strings and
//! slices) add up quickly across millions of items.
//!
//! ## Maps and Sets
//!
//! With the `hashbrown` Cargo feature, [`Arena`] can directly back
//! [`hashbrown`](https://crates.io/crates/hashbrown) collections via
//! [`Arena::alloc_hash_map`], [`Arena::alloc_hash_map_with_capacity`],
//! [`Arena::alloc_set`], and [`Arena::alloc_set_with_capacity`]. The returned
//! `HashMap` / `HashSet` store their entries in arena chunks.
//!
//! ```
//! # #[cfg(feature = "hashbrown")] {
//! use multitude::Arena;
//!
//! let arena = Arena::new();
//!
//! let mut map = arena.alloc_hash_map::<u32, &str>();
//! map.insert(1, "one");
//! assert_eq!(map.get(&1), Some(&"one"));
//!
//! let mut set = arena.alloc_set::<u32>();
//! set.insert(7);
//! assert!(set.contains(&7));
//! # }
//! ```
//!
//! # Strings
//!
//! `multitude` provides a family of arena-resident string types in the
//! [`strings`] module. The model is the same one used for arbitrary
//! values elsewhere in the crate — bump-allocation backed by a per-chunk
//! refcount — but specialized for UTF-8 / UTF-16 text and a compact
//! single-pointer representation.
//!
//! There are two roles a string type can play:
//!
//! 1. **Smart pointers (immutable / owned).** Compact handles to string
//!    data already stored in the arena. They use a single-pointer (8
//!    bytes on 64-bit) layout — half the size of `&str`. They differ in
//!    how sharing and mutability work:
//!
//!    | UTF-8 | UTF-16 | Sharing | Mutable | Notes |
//!    |---|---|---|---|---|
//!    | [`Arc<str>`](crate::Arc) | `Arc<Utf16Str>` | atomic refcount; `Clone`, `Send + Sync` | no | cross-thread sharing |
//!    | [`Box<str>`](crate::Box) | `Box<Utf16Str>` | unique owner; `Send + Sync` (not `Clone`) | yes | drops eagerly |
//!
//!    Like the other arena smart pointers, they keep their owning chunk
//!    alive via a refcount, so they can outlive the [`Arena`] they came
//!    from.
//!
//! 2. **Builders (mutable, growable).** [`String`](strings::String) and
//!    [`Utf16String`](strings::Utf16String) are transient growable
//!    buffers — small structs (32 bytes) carrying a data pointer +
//!    length + capacity + arena reference. You build them up with
//!    `push_str` / `push` / [`format!`](strings::format!) /
//!    [`format_utf16!`](strings::format_utf16!), then **freeze** them
//!    into one of the smart pointers above:
//!
//!    | Builder | Freeze method | Result |
//!    |---|---|---|
//!    | [`String`](strings::String) | [`into_boxed_str`](strings::String::into_boxed_str) | [`Box<str>`](crate::Box) |
//!    | [`Utf16String`](strings::Utf16String) | [`into_boxed_utf16_str`](strings::Utf16String::into_boxed_utf16_str) | `Box<Utf16Str>` |
//!
//!    The UTF-16 freeze reuses the buffer in place (O(1)) and reclaims any
//!    unused capacity when it can. The UTF-8 freeze copies the bytes (O(n))
//!    into a compact allocation, so [`Box<str>`](crate::Box) stays a single,
//!    `Send`-safe pointer.
//!
//! UTF-16 support requires the `utf16` Cargo feature. Strict (validated)
//! UTF-16 only — lone surrogates are rejected. The UTF-16 types
//! interoperate with `widestring::Utf16Str` / `widestring::Utf16String`
//! for I/O and FFI bridging. UTF-16 length and capacity are counted in
//! `u16` elements (matching `widestring::Utf16Str::len()`).
//!
//! ## Example: UTF-8
//!
//! ```
//! use multitude::Arena;
//! use multitude::Box;
//!
//! let arena = Arena::new();
//!
//! // Single-pointer immutable strings.
//! let s = arena.alloc_str_arc("hello, world");
//! assert_eq!(&*s, "hello, world");
//!
//! // Build incrementally and freeze:
//! let mut b = arena.alloc_string();
//! b.push_str("abc");
//! b.push_str("123");
//! let frozen: Box<str> = b.into_boxed_str();
//! assert_eq!(&*frozen, "abc123");
//!
//! // format!-style:
//! let name = "Alice";
//! let greeting = multitude::strings::format!(in &arena, "Hello, {name}!");
//! assert_eq!(&*greeting, "Hello, Alice!");
//! ```
//!
//! ## Example: UTF-16
//!
//! ```
//! # #[cfg(feature = "utf16")] {
//! use multitude::Arena;
//! use widestring::utf16str;
//!
//! let arena = Arena::new();
//!
//! // From a validated &Utf16Str literal:
//! let s = arena.alloc_utf16_str_arc(utf16str!("hello, world"));
//! assert_eq!(&*s, utf16str!("hello, world"));
//!
//! // Or transcode from a &str:
//! let s2 = arena.alloc_utf16_str_arc_from_str("hello");
//! assert_eq!(&*s2, utf16str!("hello"));
//!
//! // Build incrementally and freeze:
//! let mut b = arena.alloc_utf16_string();
//! b.push_str(utf16str!("abc"));
//! b.push_from_str("123");
//! let frozen = b.into_boxed_utf16_str();
//! assert_eq!(&*frozen, utf16str!("abc123"));
//!
//! // format!-style:
//! let name = "Alice";
//! let greeting = multitude::strings::format_utf16!(in &arena, "Hello, {name}!");
//! assert_eq!(greeting.as_utf16_str(), utf16str!("Hello, Alice!"));
//! # }
//! ```
//!
//! # Arena-Aware Deserialization
//!
//! With the `serde_json` feature, derive [`de::DeserializeIn`] on types that
//! should place owned fields in the arena, then call an [`Arena`] convenience
//! method:
//!
//! ```
//! # #[cfg(feature = "serde_json")]
//! # fn main() -> Result<(), serde_json::Error> {
//! #[derive(multitude::de::DeserializeIn)]
//! struct Request {
//!     id: u64,
//!     name: multitude::Box<str>,
//! }
//!
//! let arena = multitude::Arena::new();
//! let request: Request = arena.deserialize_json(r#"{"id":7,"name":"Ada"}"#)?;
//! assert_eq!(request.name.as_ref(), "Ada");
//! # Ok(())
//! # }
//! # #[cfg(not(feature = "serde_json"))]
//! # fn main() {}
//! ```
//!
//! Ordinary [`serde::Deserialize`] and [`de::DeserializeIn`] are independent;
//! see the [`de`] module for field compatibility, third-party types, ownership
//! choices, limits, and custom implementations.
//!
//! # Building DSTs
//!
//! With the `dst` Cargo feature enabled, [`Arena`] exposes
//! [`Arena::alloc_dst_arc`] and
//! [`Arena::alloc_dst_box`] (and their `try_*` siblings) for
//! constructing values whose layout is only known at runtime (custom
//! DSTs, fat pointers, trait objects).
//!
//! Each of these takes a [`Layout`](core::alloc::Layout), a
//! pointer-metadata value (e.g. a slice length, a `DynMetadata`), and
//! a closure that initializes the buffer through a typed fat pointer.
//! For most users, the [`dst-factory`](https://crates.io/crates/dst-factory) companion crate is the
//! recommended high-level driver; the low-level interface looks like:
//!
//! ```
//! # #[cfg(feature = "dst")] {
//! use core::alloc::Layout;
//!
//! use multitude::Arena;
//!
//! let arena = Arena::new();
//!
//! // Allocate a 5-byte slice in the arena as a `Box<[u8]>`.
//! let layout = Layout::array::<u8>(5).unwrap();
//! let b: multitude::Box<[u8]> = unsafe {
//!     arena.alloc_dst_box::<[u8]>(layout, 5, |fat: *mut [u8]| {
//!         let p = fat.cast::<u8>();
//!         for i in 0..5 {
//!             p.add(i).write(i as u8);
//!         }
//!     })
//! };
//! assert_eq!(&*b, &[0, 1, 2, 3, 4]);
//! # }
//! ```
//!
//! The same feature also enables eight `Arena::alloc_slice_*_box`
//! methods that produce `Box<[T]>` directly (mirroring the
//! existing `_arc` slice methods).
//!
//! # Crate Features
//!
//! | Feature | Description |
//! |---------|-------------|
//! | `std` *(default)* | Enables [`std::io::Write`] on [`Vec<u8>`](vec::Vec) for use with `write!`, `std::io::copy`, `serde_json::to_writer`, and similar. Disable for `#![no_std]` environments (the crate still requires `alloc`). |
//! | `stats` | Enables runtime instrumentation counters returned by `Arena::stats`. Disable for the tightest allocation throughput when you don't need observability. |
//! | `serde` | Adds `Serialize` impls for arena strings and vectors, plus arena-aware deserialization through [`de::DeserializeIn`] and [`Arena::deserialize`]. With `serde + utf16`, also adds serialization for the UTF-16 types (transcoded to UTF-8 on the wire). |
//! | `serde_json` | Implies `serde` and adds [`Arena::deserialize_json`] convenience methods with trailing-input checks and optional resource limits. |
//! | `dst` | Enables the `dst` module for constructing true dynamically-sized types and trait objects in the arena via [`Arena::alloc_dst_arc`] / [`Arena::alloc_dst_box`], plus eight `Arena::alloc_slice_*_box` methods. |
//! | `utf16` | Adds a parallel UTF-16 string surface (`Arc<Utf16Str>`, `Box<Utf16Str>`, [`Utf16String`](strings::Utf16String), and [`format_utf16!`](strings::format_utf16!)) backed by the [`widestring`](https://crates.io/crates/widestring) crate. Lengths are counted in `u16` elements. |
//! | `zerocopy` | Provides [`ZerocopyView`](zerocopy::ZerocopyView) for safe zero-initialized allocation of types implementing [`zerocopy::FromZeros`](::zerocopy::FromZeros). Access via [`Arena::zerocopy()`]. |
//! | `bytemuck` | Provides [`BytemuckView`](bytemuck::BytemuckView) for safe zero-initialized allocation of types implementing [`bytemuck::Zeroable`](::bytemuck::Zeroable). Access via [`Arena::bytemuck()`]. |
//! | `bytes` | Adds [`From`] conversions from [`Arc<[u8]>`](Arc) and [`Arc<str>`](Arc) into [`bytes::Bytes`](::bytes::Bytes), enabling zero-copy integration with the Tokio / Hyper async ecosystem. |
//! | `bytesbuf` | Implements [`bytesbuf::mem::Memory`](::bytesbuf::mem::Memory) directly on [`Arena`], so that [`BytesBuf`](::bytesbuf::BytesBuf) buffers can be backed by arena chunks. Implies `std`. |
//! | `hashbrown` | Lets [`Arena`] back [`hashbrown`](https://crates.io/crates/hashbrown) collections via [`Arena::alloc_hash_map`], [`Arena::alloc_hash_map_with_capacity`], [`Arena::alloc_set`], and [`Arena::alloc_set_with_capacity`]. |

#![no_std]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/multitude/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/multitude/favicon.ico")]

extern crate alloc;
#[cfg(any(feature = "std", test))]
extern crate std;

mod alloc_handle;
mod allocator_impl;
mod arc;
mod arena;
mod arena_builder;
#[cfg(feature = "stats")]
mod arena_stats;
mod r#box;
mod cow;
#[cfg(feature = "serde")]
#[cfg_attr(docsrs, doc(cfg(feature = "serde")))]
pub mod de;
#[cfg(feature = "dst")]
#[cfg_attr(docsrs, doc(cfg(feature = "dst")))]
pub mod dst;
mod error;
mod from_in;
mod internal;
mod rc;
pub mod strings;
mod thin_smart_ptr_common;
pub mod vec;

#[cfg(test)]
mod tests_support;

// Integrations that expose named types are public; integrations that only
// add impls and inherent methods remain private.
#[cfg(feature = "bytemuck")]
#[cfg_attr(docsrs, doc(cfg(feature = "bytemuck")))]
pub mod bytemuck;
#[cfg(feature = "bytes")]
#[cfg_attr(docsrs, doc(cfg(feature = "bytes")))]
mod bytes;
#[cfg(feature = "bytesbuf")]
#[cfg_attr(docsrs, doc(cfg(feature = "bytesbuf")))]
mod bytesbuf;
#[cfg(feature = "zerocopy")]
#[cfg_attr(docsrs, doc(cfg(feature = "zerocopy")))]
pub mod zerocopy;

pub use self::alloc_handle::Alloc;
pub use self::arc::Arc;
pub use self::arena::Arena;
pub use self::arena_builder::ArenaBuilder;
#[cfg(feature = "stats")]
#[cfg_attr(docsrs, doc(cfg(feature = "stats")))]
pub use self::arena_stats::ArenaStats;
pub use self::r#box::Box;
pub use self::cow::Cow;
pub use self::error::AllocError;
pub use self::from_in::{FromIn, IntoIn};
pub use self::rc::Rc;
