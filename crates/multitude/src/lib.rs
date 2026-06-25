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
//! `multitude` is an arena-based bump allocator designed to improve the performance of applications that have **phase-oriented logic**, which
//! is when groups of related allocations live and die together. Service request handling and parsers are two examples of this pattern which usually
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
//! 1. **Flexibility.** `multitude` provides multiple allocation styles, all of
//!    which can coexist in the same arena:
//!
//!    - Mutable references with lifetimes tied to the arena (`&mut T`,
//!      `&mut str`, `&mut [T]`).
//!    - Atomic reference-counted smart pointers ([`Arc`], [`Arc<str>`](Arc), [`Arc<[T]>`](Arc))
//!      for cross-thread sharing.
//!    - Owned, mutable smart pointers ([`Box`], [`Box<str>`](Box), [`Box<[T]>`](Box)).
//!
//! 2. **Early Reclamation.** In many situations, `multitude` can reclaim memory from individual chunks as soon as their reference counts drop to zero,
//!    without waiting for the entire arena to be dropped. This allows for more efficient memory usage in long-running arenas with many short-lived allocations.
//!
//! 3. **Smart Pointers Can Outlive the Arena.** The smart pointers produced by `multitude` can keep their owning chunk alive even after the arena itself has been dropped,
//!    allowing for more flexible memory management and longer-lived data structures.
//!
//! 4. **Drop Support.** `multitude` automatically runs `Drop` for allocated values at the appropriate time.
//!
//! 5. **Uniformly Thin Smart Pointers.** `multitude`'s [`Arc<T>`](Arc) and [`Box<T>`](Box) are **8 bytes** on 64-bit
//!    for *every* `T`.
//!
//! 6. **Efficient Mutable Strings and Vectors.** `multitude` provides [`String`](strings::String) and [`Vec`](vec::Vec) which are growable collections that live in the arena.
//!
//! 7. **Dynamically-Sized Types.** `multitude` supports dynamically-sized types (DSTs) like slices and strings, allowing you to allocate and manage them in the
//!    arena with the same flexibility as sized types. The [`dst-factory`](https://crates.io/crates/dst-factory) crate is a great companion for building DSTs in the arena.
//!
//! 8. **`format!`-style Macro.** `multitude` includes a [`format!`](strings::format!)-style macro that allows you to create formatted strings directly in the arena, avoiding intermediate allocations and copies.
//!
//! 9. **UTF-16 Support.** With the `utf16` Cargo feature, `multitude` provides a parallel set of arena-resident UTF-16 string types
//!    (`Arc<Utf16Str>`, `Box<Utf16Str>`, [`Utf16String`](strings::Utf16String)) and a [`format_utf16!`](strings::format_utf16!) macro for FFI / Windows / JS-engine
//!    interop without per-call transcoding at every boundary.
//!
//! 10. **`#![no_std]` Support.** `multitude` can be used in `#![no_std]` environments, making it suitable for embedded systems and other resource-constrained contexts.
//!
//! See [`BUMPALO.md`](https://github.com/microsoft/oxidizer/blob/main/crates/multitude/BUMPALO.md)
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
//! # Flexibility
//!
//! `multitude` supports a variety of ways to allocate data and track it over time.
//!
//! ## Simple References
//!
//! The simplest use of the arena is to get plain mutable references. The lifetime of those references is then tied
//! to the arena's own lifetime.
//!
//! ```
//! let arena = multitude::Arena::new();
//! let x: &mut u32 = arena.alloc(42);
//! let y: &mut u32 = arena.alloc(100);
//! *x += 1;
//! *y += 1;
//! assert_eq!(*x, 43);
//! assert_eq!(*y, 101);
//!
//! // Strings and slices too:
//! let s: &mut str = arena.alloc_str("hello");
//! let v: &mut [i32] = arena.alloc_slice_copy(&[1, 2, 3]);
//! ```
//!
//! These references can't outlive the arena, which limits their use. But they are the fastest and
//! most efficient way to allocate from the arena, so if the lifetime constraints are tolerable, simple
//! references are the way to go.
//!
//! ## Smart Pointers
//!
//! Smart pointers ([`Arc`], [`Box`]) work in a way similar to the like-named types
//! in the standard library, except that they reference addresses within an arena.
//!
//! ```
//! use multitude::Arc;
//!
//! struct Point {
//!     x: f64,
//!     y: f64,
//! }
//!
//! let p: Arc<Point> = {
//!     let arena = multitude::Arena::new();
//!     arena.alloc_arc(Point { x: 3.0, y: 4.0 })
//!     // arena dropped here
//! };
//! assert_eq!(p.x, 3.0);
//! ```
//!
//! Although [`Arena`] itself is `!Sync`, it is [`Send`]: an arena —
//! along with any in-flight references and smart pointers — can be
//! moved between threads. For cross-thread *sharing*, allocate
//! [`Arc`]-family smart pointers (e.g. [`Arc<u64>`](Arc), [`Arc<str>`](Arc))
//! and `.clone()` them across threads.
//!
//! ```
//! let arena = multitude::Arena::new();
//! let shared = arena.alloc_arc(42_u64);
//! let h = std::thread::spawn(move || *shared);
//! assert_eq!(42, h.join().unwrap());
//! ```
//!
//! [`Box`] is a unique owner that provides `&mut T` access, similar to
//! [`alloc::boxed::Box`] but backed by the arena.
//!
//! ```
//! let arena = multitude::Arena::new();
//! let mut v = arena.alloc_box(vec![1, 2, 3]);
//! v.push(4);
//! assert_eq!(*v, vec![1, 2, 3, 4]);
//! drop(v); // The vec drop runs here, freeing its heap buffer.
//! ```
//!
//! ## Collections
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
//! ## Freezing
//!
//! [`String`](strings::String) and [`Vec`](vec::Vec) are designed as **transient
//! builders**. They carry a data pointer + length + capacity + arena reference.
//!
//! Once you're done building, you can **freeze them** into immutable smart pointers:
//!
//! - [`String::into_boxed_str`](strings::String::into_boxed_str) →
//!   [`Box<str>`](crate::Box) (**8 bytes**, thin), or `Box::from(string)`.
//!   The freeze is **O(n)** — it copies the bytes into a compact,
//!   length-prefixed allocation so the resulting single pointer can outlive
//!   the arena. (Like any [`Box`], it is `Send`/`Sync` only when the
//!   allocator `A` is.)
//! - [`Vec::into_boxed_slice`](vec::Vec::into_boxed_slice) →
//!   [`Box<[T]>`](crate::Box) (**8 bytes**, thin), or `Box::from(vec)`.
//!   The freeze is **O(n)** — it moves the elements into a fresh compact,
//!   length-prefixed allocation so the resulting single pointer can outlive
//!   the arena. (Like any [`Box`], it is `Send`/`Sync` only when `T` and the
//!   allocator `A` are.)
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
//!    The UTF-16 freeze reuses the buffer in place (O(1)) and returns
//!    any unused tail capacity to the chunk's bump cursor when it can.
//!    The UTF-8 freeze copies the bytes (O(n)) into a compact,
//!    length-prefixed allocation so [`Box<str>`](crate::Box) stays a
//!    single, `Send`-safe pointer.
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
//! | `serde` | Adds `Serialize` impls for [`Arc<str>`](Arc), [`Box<str>`](Box), [`String`](strings::String), and [`Vec`](vec::Vec). With `serde + utf16`, also adds impls for the UTF-16 types (transcoded to UTF-8 on the wire). |
//! | `dst` | Enables the `dst` module for constructing true dynamically-sized types and trait objects in the arena via [`Arena::alloc_dst_arc`] / [`Arena::alloc_dst_box`], plus eight `Arena::alloc_slice_*_box` methods. |
//! | `utf16` | Adds a parallel UTF-16 string surface (`Arc<Utf16Str>`, `Box<Utf16Str>`, [`Utf16String`](strings::Utf16String), and [`format_utf16!`](strings::format_utf16!)) backed by the [`widestring`](https://crates.io/crates/widestring) crate. Lengths are counted in `u16` elements. |
//! | `zerocopy` | Provides [`ZerocopyView`](zerocopy::ZerocopyView) for safe zero-initialized allocation of types implementing [`zerocopy::FromZeros`](::zerocopy::FromZeros). Access via [`Arena::zerocopy()`]. |
//! | `bytemuck` | Provides [`BytemuckView`](bytemuck::BytemuckView) for safe zero-initialized allocation of types implementing [`bytemuck::Zeroable`](::bytemuck::Zeroable). Access via [`Arena::bytemuck()`]. |
//! | `bytes` | Adds [`From`] conversions from [`Arc<[u8]>`](Arc) and [`Arc<str>`](Arc) into [`bytes::Bytes`](::bytes::Bytes), enabling zero-copy integration with the Tokio / Hyper async ecosystem. |
//! | `bytesbuf` | Implements [`bytesbuf::mem::Memory`](::bytesbuf::mem::Memory) directly on [`Arena`], so that [`BytesBuf`](::bytesbuf::BytesBuf) buffers can be backed by arena chunks. Implies `std`. |
//! | `hashbrown` | Lets [`Arena`] back [`hashbrown`](https://crates.io/crates/hashbrown) collections via [`Arena::alloc_hash_map`], [`Arena::alloc_hash_map_with_capacity`], [`Arena::alloc_set`], and [`Arena::alloc_set_with_capacity`]. (`&Arena` always implements the `allocator-api2` 0.2 `Allocator` trait so it can back `hashbrown` directly; this feature adds the convenience constructors.) |

#![no_std]
#![doc(html_logo_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/multitude/logo.png")]
#![doc(html_favicon_url = "https://media.githubusercontent.com/media/microsoft/oxidizer/refs/heads/main/crates/multitude/favicon.ico")]

extern crate alloc;
#[cfg(any(feature = "std", test))]
extern crate std;

mod allocator_impl;
mod arc;
mod arena;
mod arena_builder;
#[cfg(feature = "stats")]
mod arena_stats;
mod r#box;
#[cfg(feature = "dst")]
#[cfg_attr(docsrs, doc(cfg(feature = "dst")))]
pub mod dst;
mod from_in;
mod internal;
pub mod strings;
mod thin_smart_ptr_common;
pub mod vec;

#[cfg(test)]
mod tests_support;

// Ecosystem integration modules. Visibility differs by what the
// integration exposes:
//   - `bytemuck` / `zerocopy` are `pub` because they introduce types
//     (`BytemuckView` / `ZerocopyView`) that users need to name in
//     their own code.
//   - `bytes` / `bytesbuf` are private because they only add `From`
//     impls / inherent methods on existing types; nothing in them
//     needs to be path-addressable from outside.
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

pub use self::arc::Arc;
pub use self::arena::Arena;
pub use self::arena_builder::ArenaBuilder;
#[cfg(feature = "stats")]
#[cfg_attr(docsrs, doc(cfg(feature = "stats")))]
pub use self::arena_stats::ArenaStats;
pub use self::r#box::Box;
pub use self::from_in::{FromIn, IntoIn};
