# Async File I/O Ecosystem Analysis

A detailed comparison of the **oxidizer `file` crate** against three other Rust async
file I/O libraries: **`tokio::fs`**, **`async-fs`** (smol-rs), and **`async_file`**.

---

## 1. Architecture & Threading Model

| Aspect                 | `file` (oxidizer)                                                            | `tokio::fs`                                              | `async-fs` (smol-rs)                                        | `async_file`                              |
|------------------------|------------------------------------------------------------------------------|----------------------------------------------------------|-------------------------------------------------------------|-------------------------------------------|
| **Blocking strategy**  | Dedicated per-`Directory` thread pool (1–4 threads, auto-scaling)            | Global `spawn_blocking` pool (up to 512 threads)         | `blocking` crate shared pool                                | io_uring (Linux); platform-native AIO     |
| **Thread pool scope**  | Scoped per `Root::bind` — each directory tree gets its own `Dispatcher`      | Runtime-global; shared with all `spawn_blocking` callers | Process-global; shared with all `blocking::unblock` callers | Kernel-managed submission queues          |
| **Dispatch mechanism** | `flume` channel + `async_task` crate; tasks are polled via standard `Future` | Tokio's internal `spawn_blocking` → `JoinHandle`         | `blocking::Unblock` wrapper using pipe-based async bridge   | Ring buffer submission (io_uring SQE/CQE) |
| **Runtime coupling**   | **None** — runtime-agnostic; works with tokio, smol, or any executor         | Requires tokio runtime                                   | Requires smol-compatible executor (or adapters)             | Linux-only; custom event loop             |

### Key Architectural Differences

**`file` crate**: Uses a *bounded*, dedicated thread pool (max 4 workers) per directory
tree. Workers auto-scale based on queue depth and scale down after 10 s idle. This
keeps the file I/O pool isolated from unrelated `spawn_blocking` work, preventing
noisy-neighbor effects.

**`tokio::fs`**: Shares tokio's global blocking pool. Under heavy mixed workloads
(e.g., CPU-intensive `spawn_blocking` tasks competing with file I/O), file operations
may be starved. The pool can grow to 512 threads, which helps throughput but increases
memory footprint and context-switch overhead.

**`async-fs`**: Similar to tokio but uses the `blocking` crate's auto-scaling pool.
Simpler implementation; the `Unblock` wrapper uses an internal pipe for signaling
readiness, adding a small per-operation overhead.

**`async_file`**: The only crate in this comparison that uses true kernel async I/O
(io_uring). This eliminates thread-pool overhead entirely on Linux ≥ 5.1. However, it
is Linux-only and not cross-platform.

---

## 2. API Design & Ergonomics

### 2.1 File Opening

| Feature                    | `file`                                             | `tokio::fs`                                  | `async-fs`                                   | `async_file`                   |
|----------------------------|----------------------------------------------------|----------------------------------------------|----------------------------------------------|--------------------------------|
| Open read-only             | `ReadOnlyFile::open(&dir, path)`                   | `File::open(path)`                           | `File::open(path)`                           | `File::open(path, priority)`   |
| Create write-only          | `WriteOnlyFile::create(&dir, path)`                | `File::create(path)`                         | `File::create(path)`                         | `File::create(path, priority)` |
| Open options               | `OpenOptions::new().read(true)...open(&dir, path)` | `OpenOptions::new().read(true)...open(path)` | `OpenOptions::new().read(true)...open(path)` | N/A                            |
| **Typed file handles**     | ✅ `ReadOnlyFile`, `WriteOnlyFile`, `ReadWriteFile` | ❌ Single `File` type                         | ❌ Single `File` type                         | ❌ Single `File` type           |
| **Capability-based paths** | ✅ All paths relative to `Directory`                | ❌ Absolute/arbitrary paths                   | ❌ Absolute/arbitrary paths                   | ❌ Absolute/arbitrary paths     |

The `file` crate's **type-level access control** is a unique differentiator. A
`ReadOnlyFile` physically cannot call `write()` — the method doesn't exist on the type.
This is enforced at compile time, not just by OS permissions. The other three crates use
a single `File` type where read/write errors are discovered at runtime.

The **capability-based model** (`Root::bind` → `Directory` → relative paths) prevents
path-traversal attacks by construction. Paths like `../../../etc/passwd` are rejected
before any syscall is made. No other crate in this comparison offers this.

### 2.2 Buffer Management

| Feature                    | `file`                                        | `tokio::fs`                        | `async-fs`                         | `async_file`               |
|----------------------------|-----------------------------------------------|------------------------------------|------------------------------------|----------------------------|
| **Read output type**       | `BytesView` (pooled, ref-counted)             | `Vec<u8>` or `&mut [u8]`           | `Vec<u8>` or `&mut [u8]`           | Opaque `Data` (OS-managed) |
| **Write input type**       | `BytesView` or `&[u8]`                        | `&[u8]`                            | `&[u8]`                            | `&[u8]` or `Data`          |
| **Memory pooling**         | ✅ Tiered pool (1K/4K/16K/64K via `bytesbuf`)  | ❌ Allocator-managed `Vec`          | ❌ Allocator-managed `Vec`          | ✅ OS-managed buffers       |
| **Zero-copy potential**    | ✅ Shared `BytesView` across subsystems        | ❌ Requires copy between subsystems | ❌ Requires copy between subsystems | Partial (within io_uring)  |
| **Custom memory provider** | ✅ `_with_memory` variants on all constructors | ❌                                  | ❌                                  | ❌                          |

The `file` crate's `BytesView`/`BytesBuf` system from `bytesbuf` enables a fundamentally
different data flow. Data read from a file can be handed to a network socket (or another
file) without any intermediate copies, *provided both endpoints share a memory provider*.
This is impossible with `tokio::fs` or `async-fs`, where data must be copied into/out of
`Vec<u8>` buffers at each boundary.

### 2.3 Read API Richness

| Method                  | `file`                             | `tokio::fs`             | `async-fs`              | `async_file`         |
|-------------------------|------------------------------------|-------------------------|-------------------------|----------------------|
| Read best-effort        | `read(len)` → `BytesView`          | `read(&mut buf)`        | `read(&mut buf)`        | `read(len)` → `Data` |
| Read at most N          | `read_max(len)` → `BytesView`      | `read(&mut buf[..len])` | `read(&mut buf[..len])` | N/A                  |
| Read exact              | `read_exact(len)` → `BytesView`    | `read_exact(&mut buf)`  | `read_exact(&mut buf)`  | N/A                  |
| Positional read         | `read_at(offset, len)`             | ❌ (seek + read)         | ❌ (seek + read)         | ❌                    |
| Positional read exact   | `read_exact_at(offset, len)`       | ❌                       | ❌                       | ❌                    |
| Read into `BytesBuf`    | `read_into_bytebuf(buf)`           | N/A                     | N/A                     | N/A                  |
| Read into `&mut [u8]`   | `read_into_slice(&mut buf)`        | ✅ (via `AsyncReadExt`)  | ✅ (via `AsyncReadExt`)  | ❌                    |
| Read into `MaybeUninit` | `read_exact_into_uninit(&mut buf)` | ❌                       | ❌                       | ❌                    |
| Read whole file         | `dir.read(path)`                   | `tokio::fs::read(path)` | `async_fs::read(path)`  | `file.read_all(pri)` |

The `file` crate provides **three tiers of read methods** for each I/O pattern: returning
`BytesView`, appending to `BytesBuf`, or filling a `&mut [u8]` slice. Each tier has
streaming, positional, best-effort, at-most, and exact variants. This is significantly
richer than any competitor.

**Positional I/O** (`read_at` / `write_at`) is a standout feature. These methods use
`pread`/`pwrite` (Unix) or `seek_read`/`seek_write` (Windows) to perform I/O at an
arbitrary offset *without modifying the file cursor*. This enables safe concurrent reads
from different offsets on the same file handle — impossible with the seek-then-read
pattern used by the other crates.

### 2.4 Write API

| Method                 | `file`                                                  | `tokio::fs`                    | `async-fs`                    | `async_file` |
|------------------------|---------------------------------------------------------|--------------------------------|-------------------------------|--------------|
| Write `BytesView`      | `write(data)`                                           | N/A                            | N/A                           | N/A          |
| Write `&[u8]`          | `write_slice(data)`                                     | `write_all(&data)`             | `write_all(&data)`            | N/A          |
| Positional write       | `write_at(offset, data)`                                | ❌                              | ❌                             | ❌            |
| Positional write slice | `write_slice_at(offset, data)`                          | ❌                              | ❌                             | ❌            |
| Write whole file       | `dir.write(path, view)` / `dir.write_slice(path, data)` | `tokio::fs::write(path, data)` | `async_fs::write(path, data)` | ❌            |

---

## 3. Trait Implementations

| Trait                             | `file`                                  | `tokio::fs` | `async-fs` | `async_file` |
|-----------------------------------|-----------------------------------------|-------------|------------|--------------|
| `bytesbuf_io::Read`               | ✅ (on `ReadOnlyFile`, `ReadWriteFile`)  | —           | —          | —            |
| `bytesbuf_io::Write`              | ✅ (on `WriteOnlyFile`, `ReadWriteFile`) | —           | —          | —            |
| `tokio::io::AsyncRead`            | ❌                                       | ✅           | —          | —            |
| `tokio::io::AsyncWrite`           | ❌                                       | ✅           | —          | —            |
| `tokio::io::AsyncSeek`            | ❌                                       | ✅           | —          | —            |
| `futures::AsyncRead`              | —                                       | —           | ✅          | —            |
| `futures::AsyncWrite`             | —                                       | —           | ✅          | —            |
| `futures::AsyncSeek`              | —                                       | —           | ✅          | —            |
| `std::io::Read`                   | ✅ (sync fallback)                       | ❌           | ❌          | ❌            |
| `std::io::Write`                  | ✅ (sync fallback)                       | ❌           | ❌          | ❌            |
| `std::io::Seek`                   | ✅ (sync fallback)                       | ❌           | ❌          | ❌            |
| `bytesbuf::mem::Memory`           | ✅                                       | —           | —          | —            |
| `bytesbuf::mem::HasMemory`        | ✅                                       | —           | —          | —            |
| `AsRawFd` / `AsFd` (Unix)         | ✅                                       | ✅           | ✅          | ❌            |
| `AsRawHandle` / `AsHandle` (Win)  | ✅                                       | ✅           | ❌          | —            |
| `From<ReadWriteFile>` conversions | ✅ (→ `ReadOnlyFile`, → `WriteOnlyFile`) | —           | —          | —            |

Notable: The `file` crate implements **both sync and async** I/O traits, allowing the
same file handle to be used in sync contexts (blocking the calling thread) or async
contexts. The other async crates do not provide sync trait implementations.

---

## 4. Performance Analysis

Benchmark results from the crate's `fs_comparison` benchmark suite, run on Windows with
a tokio multi-thread runtime. All times are nanoseconds per iteration (lower is better).

### 4.1 Sequential Write (one-shot `write` of entire buffer)

| Size  | `std::fs` | `tokio::fs` | `file` crate | `file` vs tokio          |
|-------|-----------|-------------|--------------|--------------------------|
| 1 KB  | 420 µs    | 561 µs      | 567 µs       | ~1.01× (parity)          |
| 64 KB | 3,044 µs  | 3,568 µs    | 3,495 µs     | ~0.98× (slightly faster) |
| 1 MB  | 5,770 µs  | 7,590 µs    | 7,546 µs     | ~0.99× (parity)          |

**Analysis**: For one-shot whole-file writes, the `file` crate and `tokio::fs` perform
nearly identically. Both pay ~30% overhead vs sync `std::fs` at small sizes (dominated
by thread-dispatch latency), converging as file size grows and actual I/O dominates.

### 4.2 Sequential Read (one-shot `read` of entire file)

| Size  | `std::fs` | `tokio::fs` | `file` crate | `file` vs tokio |
|-------|-----------|-------------|--------------|-----------------|
| 1 KB  | 92 µs     | 146 µs      | 155 µs       | ~1.06×          |
| 64 KB | 119 µs    | 184 µs      | 913 µs       | ~4.96× slower   |
| 1 MB  | 591 µs    | 645 µs      | 1,210 µs     | ~1.88× slower   |

**Analysis**: The `file` crate's sequential read path shows higher overhead at mid-range
sizes. This is attributable to the `bytesbuf` pooled buffer allocation and multi-dispatch
read loop (the crate reads in 8 KB chunks by default even for `dir.read()`, issuing
multiple dispatch round-trips). For 64 KB files, this means ~8 dispatches vs tokio's
single `spawn_blocking` call that reads the entire file at once.

*Trade-off*: The `file` crate pays more per-read for the benefits of pooled memory and
zero-copy `BytesView` output. Applications that subsequently forward the data (e.g., to
a network socket) recoup this cost by avoiding a full-buffer copy at the next boundary.

### 4.3 Streaming Read (1 MB file in 8 KB chunks)

| Implementation | Time     | vs `std::fs` |
|----------------|----------|--------------|
| `std::fs`      | 516 µs   | 1.0×         |
| `tokio::fs`    | 5,660 µs | 11.0×        |
| `file` crate   | 5,793 µs | 11.2×        |

**Analysis**: Both async crates perform nearly identically for chunked streaming reads.
The ~11× overhead vs sync is dominated by the per-chunk dispatch cost (128 round-trips
for a 1 MB file). This is inherent to the thread-pool dispatch model and would only be
eliminated by kernel-level async I/O (io_uring).

### 4.4 Streaming Write (128 × 8 KB chunks)

| Implementation | Time     | vs `std::fs` |
|----------------|----------|--------------|
| `std::fs`      | 6,292 µs | 1.0×         |
| `tokio::fs`    | 7,335 µs | 1.17×        |
| `file` crate   | 7,178 µs | 1.14×        |

**Analysis**: The `file` crate is ~2% faster than `tokio::fs` for streaming writes.
Write overhead is lower than read overhead because writes are fire-and-forget (the OS
buffers them in the page cache), so the dispatch round-trip is less impactful.

### 4.5 Many Small Files (100 × 256-byte files: create + write + read + delete)

| Implementation | Time     | vs `std::fs` |
|----------------|----------|--------------|
| `std::fs`      | 140.3 ms | 1.0×         |
| `tokio::fs`    | 154.0 ms | 1.10×        |
| `file` crate   | 153.5 ms | 1.09×        |

**Analysis**: Essentially identical. For metadata-heavy workloads, the bottleneck is
filesystem syscall latency, not dispatch overhead.

### 4.6 Metadata (100 × `stat` calls)

| Implementation | Time     | vs `std::fs` |
|----------------|----------|--------------|
| `std::fs`      | 2,900 µs | 1.0×         |
| `tokio::fs`    | 7,496 µs | 2.58×        |
| `file` crate   | 8,979 µs | 3.10×        |

**Analysis**: The `file` crate is ~20% slower than tokio for metadata operations. This
is likely due to the smaller, dedicated thread pool (max 4 workers vs tokio's 512) and
the additional path validation (`safe_join`) on each call.

### 4.7 Performance Summary

```
Benchmark               std::fs    tokio::fs    file crate    Winner (async)
──────────────────────  ─────────  ───────────  ────────────  ──────────────
Seq. Write 1 KB           420 µs      561 µs       567 µs    tokio (~1%)
Seq. Write 64 KB        3,044 µs    3,568 µs     3,495 µs    file  (~2%)
Seq. Write 1 MB         5,770 µs    7,590 µs     7,546 µs    file  (~1%)
Seq. Read 1 KB             92 µs      146 µs       155 µs    tokio (~6%)
Seq. Read 64 KB           119 µs      184 µs       913 µs    tokio (~5×)
Seq. Read 1 MB            591 µs      645 µs     1,210 µs    tokio (~2×)
Streaming Read 1 MB       516 µs    5,660 µs     5,793 µs    tokio (~2%)
Streaming Write 1 MB    6,292 µs    7,335 µs     7,178 µs    file  (~2%)
Many Small Files        140.3 ms    154.0 ms     153.5 ms    file  (~0.3%)
Metadata ×100           2,900 µs    7,496 µs     8,979 µs    tokio (~17%)
```

---

## 5. Unique Features & Limitations

### 5.1 `file` Crate — Unique Strengths

| Feature                       | Description                                                                         |
|-------------------------------|-------------------------------------------------------------------------------------|
| **Capability-based access**   | Path traversal attacks prevented by design; no absolute paths after `Root::bind`    |
| **Type-level access control** | `ReadOnlyFile` / `WriteOnlyFile` / `ReadWriteFile` enforced at compile time         |
| **Pooled buffer management**  | Tiered memory pool avoids per-read allocation; enables zero-copy data pipelines     |
| **Custom memory providers**   | `_with_memory` variants allow cross-subsystem zero-copy (e.g., file → network)      |
| **Positional I/O**            | `read_at` / `write_at` using `pread`/`pwrite` without cursor mutation               |
| **MaybeUninit reads**         | `read_exact_into_uninit` for stack-allocated or pre-allocated uninitialized buffers |
| **Runtime-agnostic**          | No dependency on tokio, smol, or any specific async runtime                         |
| **Sync fallback**             | `std::io::Read/Write/Seek` implementations for blocking contexts                    |
| **Scoped thread pool**        | Isolated 1–4 thread pool per directory tree; prevents noisy-neighbor effects        |
| **File locking**              | `lock()`, `lock_shared()`, `try_lock()`, `unlock()` — async file locking API        |

### 5.2 `file` Crate — Limitations

| Limitation                       | Detail                                                                                                 |
|----------------------------------|--------------------------------------------------------------------------------------------------------|
| **Sequential read overhead**     | Multi-dispatch chunked reads are slower than tokio's single-shot `spawn_blocking` for whole-file reads |
| **Small thread pool**            | Max 4 workers may bottleneck under extreme concurrent I/O; tokio allows up to 512                      |
| **No `AsyncRead`/`AsyncWrite`**  | Does not implement tokio or futures async traits; cannot be plugged into `tokio::io::copy` etc.        |
| **`BytesView` learning curve**   | Unfamiliar buffer types for developers used to `Vec<u8>` / `&[u8]` patterns                            |
| **Mandatory `Directory` handle** | No way to open a file by absolute path directly; always requires `Root::bind` first                    |
| **No `Stream`/`AsyncIterator`**  | `ReadDir` uses `next_entry()` loop, not `Stream` trait                                                 |

### 5.3 `tokio::fs` — Strengths & Limitations

| Strengths                                                  | Limitations                                           |
|------------------------------------------------------------|-------------------------------------------------------|
| Ecosystem standard; works with all tokio-based libraries   | No capability-based access control                    |
| `AsyncRead`/`AsyncWrite`/`AsyncSeek` trait implementations | No pooled buffer management; allocates `Vec` per read |
| Large thread pool scales to heavy concurrent I/O           | Thread pool is shared with all `spawn_blocking` users |
| Simple, familiar `std::fs`-like API                        | No type-level read/write distinction                  |
|                                                            | No positional I/O (`pread`/`pwrite`)                  |
|                                                            | Tightly coupled to tokio runtime                      |

### 5.4 `async-fs` (smol-rs) — Strengths & Limitations

| Strengths                                           | Limitations                                                   |
|-----------------------------------------------------|---------------------------------------------------------------|
| Lightweight; minimal dependencies                   | Same limitations as tokio::fs (no pooling, no positional I/O) |
| Runtime-agnostic (works with smol, async-std, etc.) | Uses pipe-based `Unblock` signaling (slight overhead)         |
| Mirrors `std::fs` API exactly                       | No Windows handle traits (`AsRawHandle`)                      |
| `futures` ecosystem trait compatibility             | Limited community adoption vs tokio                           |
|                                                     | No file locking API                                           |

### 5.5 `async_file` — Strengths & Limitations

| Strengths                                                      | Limitations                                                   |
|----------------------------------------------------------------|---------------------------------------------------------------|
| True kernel async I/O via io_uring (zero thread-pool overhead) | **Linux-only** — not cross-platform                           |
| Priority-based scheduling for I/O operations                   | Single in-flight operation per file handle                    |
| OS-managed memory prevents use-after-free                      | Opaque `Data` type — harder to integrate with byte-slice APIs |
| Lowest possible per-op latency on supported platforms          | Limited ecosystem integration (no `AsyncRead`/`AsyncWrite`)   |
|                                                                | Requires Linux kernel ≥ 5.1                                   |
|                                                                | Small community; minimal documentation                        |

---

## 6. Feature Matrix Summary

| Feature                   | `file` | `tokio::fs` | `async-fs` | `async_file` |
|---------------------------|:------:|:-----------:|:----------:|:------------:|
| Cross-platform            |   ✅    |      ✅      |     ✅      |  ❌ (Linux)   |
| Runtime-agnostic          |   ✅    |  ❌ (tokio)  |     ✅      |  ❌ (custom)  |
| Capability-based security |   ✅    |      ❌      |     ❌      |      ❌       |
| Type-level access control |   ✅    |      ❌      |     ❌      |      ❌       |
| Pooled memory / zero-copy |   ✅    |      ❌      |     ❌      |   Partial    |
| Custom memory providers   |   ✅    |      ❌      |     ❌      |      ❌       |
| Positional I/O            |   ✅    |      ❌      |     ❌      |      ❌       |
| Async file locking        |   ✅    |      ❌      |     ❌      |      ❌       |
| Sync trait fallback       |   ✅    |      ❌      |     ❌      |      ❌       |
| `AsyncRead`/`AsyncWrite`  |   ❌    |      ✅      |     ✅      |      ❌       |
| Priority scheduling       |   ❌    |      ❌      |     ❌      |      ✅       |
| True kernel async I/O     |   ❌    |      ❌      |     ❌      |      ✅       |
| Directory operations      |   ✅    |      ✅      |     ✅      |      ❌       |
| Symlink handling          |   ✅    |      ✅      |     ✅      |      ❌       |
| `OpenOptions` builder     |   ✅    |      ✅      |     ✅      |      ❌       |
| `DirBuilder`              |   ✅    |      ✅      |     ✅      |      ❌       |
| `MaybeUninit` reads       |   ✅    |      ❌      |     ❌      |      ❌       |
| Raw fd/handle access      |   ✅    |      ✅      |  ✅ (Unix)  |      ❌       |

---

## 7. Recommendations

**Choose the `file` crate when:**
- Security is paramount (capability-based path sandboxing)
- Building data pipelines where zero-copy buffer sharing matters
- You need positional I/O for concurrent reads at different offsets
- Runtime independence is required
- Type-level enforcement of read vs write access is desired

**Choose `tokio::fs` when:**
- Already committed to the tokio ecosystem
- Need `AsyncRead`/`AsyncWrite` compatibility with tokio combinators
- Simple, well-documented API is preferred
- Maximum raw sequential-read throughput is needed (single-shot reads)

**Choose `async-fs` when:**
- Using the smol runtime or need runtime-agnostic `futures` trait compatibility
- Want the lightest possible async fs wrapper
- Need a 1:1 mapping to `std::fs` with minimal learning curve

**Choose `async_file` when:**
- Targeting Linux exclusively and need the absolute lowest latency
- I/O priority scheduling is a requirement
- True kernel-level async I/O (io_uring) is needed to avoid thread-pool overhead
