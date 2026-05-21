# Comparison with `bumpalo`

[`bumpalo`](https://crates.io/crates/bumpalo) is the closest crate in
spirit; here's how multitude differs.

| Capability                                              | `bumpalo`                      | `multitude`                                                                                                                                     |
|---------------------------------------------------------|--------------------------------|-------------------------------------------------------------------------------------------------------------------------------------------------|
| Bump allocation                                         | ✅                              | ✅                                                                                                                                               |
| Simple references (`&'arena mut T`)                     | ✅ `Bump::alloc`                | ✅ `Arena::alloc`                                                                                                                                |
| `Allocator` trait integration                           | ✅ via `allocator-api2`         | ✅ via `allocator-api2`                                                                                                                          |
| Reclamation granularity                                 | Whole arena at reset           | **Per chunk**, as refcounts hit 0 (refcount smart pointers); whole-arena (simple references)                                                    |
| Smart pointers                                          | ❌ (raw `&'bump T`)             | ✅ `Rc`, `Arc`, `RcStr`                                                                                                                          |
| Smart pointers outlive the arena                        | ❌                              | ✅ (`Rc` / `Arc` / `Box` and their `str` variants — simple references are lifetime-bound)                                                        |
| Cross-thread sharing of individual values               | ❌                              | ✅ via `Arc`                                                                                                                                     |
| Automatic per-object `Drop`                             | Only via `bumpalo::boxed::Box` | ✅ Automatic (refcount smart pointers drop at chunk teardown; `Box` / `BoxStr` drop at smart pointer drop; simple references drop at arena drop) |
| Owned single smart pointer (`Drop` on drop)             | `bumpalo::boxed::Box`          | `Box`                                                                                                                                           |
| Single-pointer string smart pointers                    | ❌ (`&str` is 16 bytes)         | ✅ `RcStr` / `ArcStr` / `BoxStr` are 8 bytes                                                                                                     |
| Growable collections                                    | ✅ `bumpalo::collections`       | ✅ `Vec` (in `multitude::vec`), `String` (in `multitude::strings`)                                                                               |
| `format!`-style macro                                   | ✅                              | ✅                                                                                                                                               |
| UTF-16 strings                                          | ❌                              | ✅ via `RcUtf16Str` / `ArcUtf16Str` / `BoxUtf16Str` / `Utf16String` (gated on the `utf16` feature)                                               |
| Dynamically-sized types (DSTs, e.g. `dyn Trait`, `[T]`) | ❌                              | ✅ via the `dst` module (gated on the `dst` feature)                                                                                             |
| `zerocopy` integration                                  | ❌                              | ✅ Zero-initialized allocations for `FromZeros` types (gated on the `zerocopy` feature)                                                          |
| `bytemuck` integration                                  | ❌                              | ✅ Zero-initialized allocations for `Zeroable` types (gated on the `bytemuck` feature)                                                           |
| `bytesbuf` integration                                  | ❌                              | ✅ Arena implements `bytesbuf::mem::Memory` for arena-backed byte buffers (gated on the `bytesbuf` feature)                                      |
| `#![no_std]`                                            | ✅                              | ✅                                                                                                                                               |
