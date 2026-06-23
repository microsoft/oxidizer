# Comparison with `bumpalo`

[`bumpalo`](https://crates.io/crates/bumpalo) is the closest crate in
spirit; here's how multitude differs.

| Capability                                        | `bumpalo`                  | `multitude`                                                                                                                         |
|---------------------------------------------------|----------------------------|-------------------------------------------------------------------------------------------------------------------------------------|
| Bump allocation                                   | ✅                          | ✅                                                                                                                                   |
| Simple references (`&'arena mut T`)               | ✅ `Bump::alloc`            | ✅ `Arena::alloc`                                                                                                                    |
| `Allocator` trait integration                     | ✅ via `allocator-api2`     | ✅ via `allocator-api2`                                                                                                              |
| Reclamation granularity                           | Whole arena at reset       | **Per chunk**, as refcounts hit 0 (refcount smart pointers); whole-arena (simple references)                                        |
| `Drop` trait support                              | `Drop` runs only for `Box` | ✅ Automatic (`Arc` drops at last-clone drop, `Box` drops at smart-pointer drop, simple references drop at arena reset/drop)         |
| Owned single smart pointer (`Box`)                | ✅                          | ✅                                                                                                                                   |
| Growable collections                              | ✅ `bumpalo::collections`   | ✅ `Vec`, `String`, `Utf16String`                                                                                                    |
| `format!`-style macro                             | ✅                          | ✅                                                                                                                                   |
| `#![no_std]`                                      | ✅                          | ✅                                                                                                                                   |
| Smart-pointer width                               | 16 bytes                   | 8 bytes                                                                                                                             |
| Refcounted smart pointers (`Arc`)                 | ❌                          | ✅ `Arc`                                                                                                                             |
| Smart pointers outlive the arena                  | ❌                          | ✅ (`Arc` / `Box` and their `str` variants — simple references are lifetime-bound)                                                   |
| Cross-thread sharing of individual values         | ❌                          | ✅ via `Arc`                                                                                                                         |
| In-place growth of `Vec` / `String`               | ❌ No copy-free growth      | ✅                                                                                                                                   |
| Freeze a `Vec` into an owned `Box`/`Arc` slice    | ❌ (only a `&[T]` leak via `into_bump_slice`) | ✅ Zero-copy `Vec::into_boxed_slice` / `Arc::from` reuse the buffer in place (drop-capable, refcounted)             |
| Single-pointer string smart pointers              | ❌ (`&str` is 16 bytes)     | ✅ `Arc<str>` / `Box<str>` / `Arc<Utf16Str>` / `Box<Utf16Str>` are all 8 bytes                                                       |
| UTF-16 strings                                    | ❌                          | ✅ via `Arc<Utf16Str>` / `Box<Utf16Str>` / `Utf16String` (gated on the `utf16` feature)                                              |
| Dynamically-sized types (e.g. `dyn Trait`, `[T]`) | ❌                          | ✅ via the `dst` module (gated on the `dst` feature)                                                                                 |
| `zerocopy` integration                            | ❌                          | ✅ Zero-initialized allocations for `FromZeros` types (gated on the `zerocopy` feature)                                              |
| `bytemuck` integration                            | ❌                          | ✅ Zero-initialized allocations for `Zeroable` types (gated on the `bytemuck` feature)                                               |
| `bytes` integration                               | ❌                          | ✅ `From<Arc<[u8]>>` / `From<Arc<str>>` into `bytes::Bytes` for zero-copy Tokio / Hyper interop (gated on the `bytes` feature)       |
| `bytesbuf` integration                            | ❌                          | ✅ Arena implements `bytesbuf::mem::Memory` for arena-backed byte buffers (gated on the `bytesbuf` feature)                          |
| `serde` integration                               | ❌                          | ✅ `Serialize` impls for `Arc<str>`, `Box<str>`, `String`, `Vec` (+ UTF-16 types with `serde + utf16`); gated on the `serde` feature |
| Runtime allocation statistics                     | ❌                          | ✅ `Arena::stats()` returns chunk counts, total bytes, and relocation counters (gated on the `stats` feature)                        |
