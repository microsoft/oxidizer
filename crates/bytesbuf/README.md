<div align="center">
 <img src="./logo.png" alt="Bytesbuf Logo" width="96">

# Bytesbuf

[![crate.io](https://img.shields.io/crates/v/bytesbuf.svg)](https://crates.io/crates/bytesbuf)
[![docs.rs](https://docs.rs/bytesbuf/badge.svg)](https://docs.rs/bytesbuf)
[![MSRV](https://img.shields.io/crates/msrv/bytesbuf)](https://crates.io/crates/bytesbuf)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](../../LICENSE)
<a href="../.."><img src="../../logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Create and manipulate byte sequences for efficient I/O.

A byte sequence is a logical sequence of zero or more bytes stored in memory,
similar to a slice `&[u8]` but with some key differences:

* The bytes in a byte sequence are not required to be consecutive in memory.
* The bytes in a byte sequence are always immutable.

In practical terms, you may think of a byte sequence as a `Vec<Vec<u8>>` whose contents are
treated as one logical sequence of bytes. Byte sequences are created via [`BytesBuf`][__link0] and
consumed via [`BytesView`][__link1].

## Consuming Byte Sequences

A byte sequence is typically consumed by reading its contents. This is done via the
[`BytesView`][__link2] type, which is a view over a byte sequence. When reading data, the read
bytes are removed from the view, shrinking it to only the remaining bytes.

There are many helper methods on this type for easily consuming bytes from the view:

* [`get_num_le::<T>()`][__link3] reads numbers. Big-endian/native-endian variants also exist.
* [`get_byte()`][__link4] reads a single byte.
* [`copy_to_slice()`][__link5] copies bytes into a provided slice.
* [`copy_to_uninit_slice()`][__link6] copies bytes into a provided uninitialized slice.
* [`as_read()`][__link7] creates a `std::io::Read` adapter for reading bytes via standard I/O methods.

```rust
use bytesbuf::BytesView;

fn consume_message(mut message: BytesView) {
    // We read the message and calculate the sum of all the words in it.
    let mut sum: u64 = 0;

    while !message.is_empty() {
        let word = message.get_num_le::<u64>();
        sum = sum.saturating_add(word);
    }

    println!("Message received. The sum of all words in the message is {sum}.");
}
```

If the helper methods are not sufficient, you can access the byte sequence via byte slices using the
following fundamental methods that underpin the convenience methods:

* [`first_slice()`][__link8], which returns the first slice of bytes that makes up the byte sequence. The
  length of this slice is determined by the inner structure of the byte sequence and it may not
  contain all the bytes.
* [`advance()`][__link9], which marks bytes from the beginning of [`first_slice()`][__link10] as read, shrinking the
  view of the byte sequence by the corresponding amount and moving remaining data up to the front.
  When you advance past the slice returned by [`first_slice()`][__link11], the next call to [`first_slice()`][__link12]
  will return a new slice of bytes starting from the new front position of the view.

```rust
use bytesbuf::BytesView;

let len = bytes.len();
let mut slice_lengths = Vec::new();

while !bytes.is_empty() {
    let slice = bytes.first_slice();
    slice_lengths.push(slice.len());

    // We have completed processing this slice. All we wanted was to know its length.
    // We can now mark this slice as consumed, revealing the next slice for inspection.
    bytes.advance(slice.len());
}

println!("Inspected a view over {len} bytes with slice lengths: {slice_lengths:?}");
```

To reuse a byte sequence, clone it before consuming the contents. This is a cheap
zero-copy operation.

```rust
use bytesbuf::BytesView;

assert_eq!(bytes.len(), 16);

let mut bytes_clone = bytes.clone();
assert_eq!(bytes_clone.len(), 16);

// Consume 8 bytes from the front.
_ = bytes_clone.get_num_le::<u64>();
assert_eq!(bytes_clone.len(), 8);

// Operations on the clone have no effect on the original view.
assert_eq!(bytes.len(), 16);
```

## Producing Byte Sequences

For creating a byte sequence, you first need some memory capacity to put the bytes into. This
means you need a memory provider, which is a type that implements the [`Memory`][__link13] trait.

Obtaining a memory provider is generally straightforward. Simply use the first matching option
from the following list:

1. If you are creating byte sequences for the purpose of submitting them to a specific
   object of a known type (e.g. writing them to a `TcpConnection`), the target type will
   typically implement the [`HasMemory`][__link14] trait, which gives you a suitable memory
   provider instance via [`HasMemory::memory()`][__link15]. Use this as the memory provider - it will
   give you memory with the configuration that is optimal for delivering bytes to that
   specific consumer.
1. If you are creating byte sequences as part of usage-neutral data processing, obtain an
   instance of a shared [`GlobalPool`][__link16]. In a typical web application, the global memory pool
   is a service exposed by the application framework. In a different context (e.g. example
   or test code with no framework), you can create your own instance via `GlobalPool::new()`.

Once you have a memory provider, you can reserve memory from it by calling
[`Memory::reserve()`][__link17] on it. This returns a [`BytesBuf`][__link18] with the requested
memory capacity.

```rust
use bytesbuf::Memory;

let memory = connection.memory();

let mut buf = memory.reserve(100);
```

Now that you have the memory capacity in a [`BytesBuf`][__link19], you can fill the memory
capacity with bytes of data. Creating byte sequences in a [`BytesBuf`][__link20] is an
append-only process - you can only add data to the end of the buffered sequence.

There are many helper methods on [`BytesBuf`][__link21] for easily appending bytes to the buffer:

* [`put_num_le::<T>()`][__link22], which appends numbers. Big-endian/native-endian variants also exist.
* [`put_slice()`][__link23], which appends a slice of bytes.
* [`put_byte()`][__link24], which appends a single byte.
* [`put_byte_repeated()`][__link25], which appends multiple repetitions of a byte.
* [`put_bytes()`][__link26], which appends an existing [`BytesView`][__link27].

```rust
use bytesbuf::Memory;

let memory = connection.memory();

let mut buf = memory.reserve(100);

buf.put_num_be(1234_u64);
buf.put_num_be(5678_u64);
buf.put_slice(*b"Hello, world!");
```

If the helper methods are not sufficient, you can write contents directly into mutable byte slices
using the fundamental methods that underpin the convenience methods:

* [`first_unfilled_slice()`][__link28], which returns a mutable slice of bytes from the beginning of the
  buffer’s remaining capacity. The length of this slice is determined by the inner memory layout
  of the buffer and it may not contain all the capacity that has been reserved.
* [`advance()`][__link29], which declares that a number of bytes at the beginning of [`first_unfilled_slice()`][__link30]
  have been initialized with data and are no longer unused. This will mark these bytes as valid for
  consumption and advance [`first_unfilled_slice()`][__link31] to the next slice of unused memory capacity
  if the current slice has been completely filled.

See `examples/bb_slice_by_slice_write.rs` for an example of how to use these methods.

If you do not know exactly how much memory you need in advance, you can extend the sequence
builder capacity on demand by calling [`BytesBuf::reserve()`][__link32]. You can use [`remaining_capacity()`][__link33]
to identify how much unused memory capacity is available.

```rust
use bytesbuf::Memory;

let memory = connection.memory();

let mut buf = memory.reserve(100);

// .. write some data into the buffer ..

// We discover that we need 80 additional bytes of memory! No problem.
buf.reserve(80, &memory);

// Remember that a memory provider can always provide more memory than requested.
assert!(buf.capacity() >= 100 + 80);
assert!(buf.remaining_capacity() >= 80);
```

When you have filled the memory capacity with the contents of the byte sequence, you can consume
the data in the buffer as a [`BytesView`][__link34] over immutable bytes.

```rust
use bytesbuf::Memory;

let memory = connection.memory();

let mut buf = memory.reserve(100);

buf.put_num_be(1234_u64);
buf.put_num_be(5678_u64);
buf.put_slice(*b"Hello, world!");

let message = buf.consume_all();
```

This can be done piece by piece, and you can continue writing to the buffer
after consuming some already written bytes.

```rust
use bytesbuf::Memory;

let memory = connection.memory();

let mut buf = memory.reserve(100);

buf.put_num_be(1234_u64);
buf.put_num_be(5678_u64);

let first_8_bytes = buf.consume(8);
let second_8_bytes = buf.consume(8);

buf.put_slice(*b"Hello, world!");

let final_contents = buf.consume_all();
```

If you already have a [`BytesView`][__link35] that you want to write into a [`BytesBuf`][__link36], call
[`BytesBuf::put_bytes()`][__link37]. This is a highly efficient zero-copy operation
that reuses the memory capacity of the view you are appending.

```rust
use bytesbuf::Memory;

let memory = connection.memory();

let mut header_builder = memory.reserve(16);
header_builder.put_num_be(1234_u64);
let header = header_builder.consume_all();

let mut buf = memory.reserve(128);
buf.put_bytes(header);
buf.put_slice(*b"Hello, world!");
```

Note that there is no requirement that the memory capacity of the buffer and the
memory capacity of the view being appended come from the same memory provider. It is valid
to mix and match memory from different providers, though this may disable some optimizations.

## Implementing APIs that Consume Byte Sequences

If you are implementing a type that accepts byte sequences, you should implement the
[`HasMemory`][__link38] trait to make it possible for the caller to use optimally
configured memory when creating the byte sequences for input to your type.

Even if the implementation of your type today is not capable of taking advantage of
optimizations that depend on the memory configuration, it may be capable of doing so
in the future or may, today or in the future, pass the data to another type that
implements [`HasMemory`][__link39], which can take advantage of memory optimizations.
Therefore, it is best to implement this trait on all types that accept byte sequences.

The recommended implementation strategy for [`HasMemory`][__link40] is as follows:

* If your type always passes the data to another type that implements [`HasMemory`][__link41],
  simply forward the memory provider from the other type.
* If your type can take advantage of optimizations enabled by specific memory configurations,
  (e.g. because it uses operating system APIs that unlock better performance when the memory
  is appropriately configured), return a memory provider that performs the necessary
  configuration.
* If your type neither passes the data to another type that implements [`HasMemory`][__link42]
  nor can take advantage of optimizations enabled by specific memory configurations, obtain
  an instance of [`GlobalPool`][__link43] as a dependency and return it as the memory provider.

Example of forwarding the memory provider (see `examples/bb_has_memory_forwarding.rs`
for full code):

```rust
use bytesbuf::{HasMemory, MemoryShared, BytesView};

/// Counts the number of 0x00 bytes in a byte sequence before
/// writing that byte sequence to a network connection.
///
/// # Implementation strategy for `HasMemory`
///
/// This type merely inspects a byte sequence before passing it on. This means that it does not
/// have a preference of its own for how that memory should be configured.
///
/// However, the thing it passes the sequence to (the `Connection` type) may have a preference,
/// so we forward the memory provider of the `Connection` type as our own memory provider, so the
/// caller can use memory optimal for submission to the `Connection` instance.
#[derive(Debug)]
struct ConnectionZeroCounter {
    connection: Connection,
}

impl ConnectionZeroCounter {
    pub fn new(connection: Connection) -> Self {
        Self {
            connection,
        }
    }

    pub fn write(&mut self, message: BytesView) {
        // TODO: Count zeros.

        self.connection.write(message);
    }
}

impl HasMemory for ConnectionZeroCounter {
    fn memory(&self) -> impl MemoryShared {
        // We forward the memory provider of the connection, so that the caller can use
        // memory optimal for submission to the connection.
        self.connection.memory()
    }
}
```

Example of returning a memory provider that performs configuration for optimal memory (see
`examples/bb_has_memory_optimizing.rs` for full code):

```rust
use bytesbuf::{CallbackMemory, HasMemory, MemoryShared, BytesView};

/// # Implementation strategy for `HasMemory`
///
/// This type can benefit from optimal performance if specifically configured memory is used and
/// the memory is reserved from the I/O memory pool. It uses the I/O context to reserve memory,
/// providing a usage-specific configuration when reserving memory capacity.
///
/// A callback memory provider is used to attach the configuration to each memory reservation.
#[derive(Debug)]
struct UdpConnection {
    io_context: IoContext,
}

impl UdpConnection {
    pub fn new(io_context: IoContext) -> Self {
        Self { io_context }
    }
}

/// Represents the optimal memory configuration for a UDP connection when reserving I/O memory.
const UDP_CONNECTION_OPTIMAL_MEMORY_CONFIGURATION: MemoryConfiguration = MemoryConfiguration {
    requires_page_alignment: false,
    zero_memory_on_release: false,
    requires_registered_memory: true,
};

impl HasMemory for UdpConnection {
    fn memory(&self) -> impl MemoryShared {
        CallbackMemory::new({
            // Cloning is cheap, as it is a service that shares resources between clones.
            let io_context = self.io_context.clone();

            move |min_len| {
                io_context.reserve_io_memory(min_len, UDP_CONNECTION_OPTIMAL_MEMORY_CONFIGURATION)
            }
        })
    }
}

```

Example of returning a global memory pool when the type is agnostic toward memory configuration
(see `examples/bb_has_memory_global.rs` for full code):

```rust
use bytesbuf::{GlobalPool, HasMemory, MemoryShared};

/// Calculates a checksum for a given byte sequence.
///
/// # Implementation strategy for `HasMemory`
///
/// This type does not benefit from any specific memory configuration - it consumes bytes no
/// matter what sort of memory they are in. It also does not pass the bytes to some other type.
///
/// Therefore, we simply use `GlobalPool` as the memory provider we publish, as this is
/// the default choice when there is no specific provider to prefer.
#[derive(Debug)]
struct ChecksumCalculator {
    // The application logic must provide this - it is our dependency.
    memory: GlobalPool,
}

impl ChecksumCalculator {
    pub fn new(memory: GlobalPool) -> Self {
        Self { memory }
    }
}

impl HasMemory for ChecksumCalculator {
    fn memory(&self) -> impl MemoryShared {
        // Cloning a memory provider is intended to be a cheap operation, reusing resources.
        self.memory.clone()
    }
}
```

It is generally expected that all APIs work with byte sequences using memory from any provider.
It is true that in some cases this may be impossible (e.g. because you are interacting directly
with a device driver that requires the data to be in a specific physical memory module) but
these cases will be rare and must be explicitly documented.

If your type can take advantage of optimizations enabled by specific memory configurations,
it needs to determine whether a byte sequence actually uses the desired memory configuration.
This can be done by inspecting the provided byte sequence and the memory metadata it exposes.
If the metadata indicates a suitable configuration, the optimal implementation can be used.
Otherwise, the implementation can fall back to a generic implementation that works with any
byte sequence.

Example of identifying whether a byte sequence uses the optimal memory configuration (see
`examples/bb_optimal_path.rs` for full code):

```rust
use bytesbuf::BytesView;

pub fn write(&mut self, message: BytesView) {
    // We now need to identify whether the message actually uses memory that allows us to
    // ues the optimal I/O path. There is no requirement that the data passed to us contains
    // only memory with our preferred configuration.

    let use_optimal_path = message.iter_slice_metas().all(|meta| {
        // If there is no metadata, the memory is not I/O memory.
        meta.is_some_and(|meta| {
            // If the type of metadata does not match the metadata
            // exposed by the I/O memory provider, the memory is not I/O memory.
            let Some(io_memory_configuration) = meta.downcast_ref::<MemoryConfiguration>()
            else {
                return false;
            };

            // If the memory is I/O memory but is not not pre-registered
            // with the operating system, we cannot use the optimal path.
            io_memory_configuration.requires_registered_memory
        })
    });

    if use_optimal_path {
        self.write_optimal(message);
    } else {
        self.write_fallback(message);
    }
}
```

Note that there is no requirement that a byte sequence consists of homogeneous memory. Different
parts of the byte sequence may come from different memory providers, so all chunks must be
checked for compatibility.

## Compatibility with the `bytes` Crate

The popular [`Bytes`][__link44] type from the `bytes` crate is often used in the Rust ecosystem to
represent simple byte buffers of consecutive bytes. For compatibility with this commonly used
type, this crate offers conversion methods to translate between [`BytesView`][__link45] and [`Bytes`][__link46]:

* [`BytesView::to_bytes()`][__link47] converts a [`BytesView`][__link48] into a [`Bytes`][__link49] instance. This
  is not always zero-copy because a byte sequence is not guaranteed to be consecutive in memory.
  You are discouraged from using this method in any performance-relevant logic path.
* `BytesView::from(Bytes)` or `let s: BytesView = bytes.into()` converts a [`Bytes`][__link50] instance
  into a [`BytesView`][__link51]. This is an efficient zero-copy operation that reuses the memory of the
  `Bytes` instance.

## Static Data

You may have static data in your logic, such as the names/prefixes of request/response headers:

```rust
const HEADER_PREFIX: &[u8] = b"Unix-Milliseconds: ";
```

Optimal processing of static data requires satisfying multiple requirements:

* We want zero-copy processing when consuming this data.
* We want to use memory that is optimally configured for the context in which the data is
  consumed (e.g. network connection, file, etc).

The standard pattern here is to use [`OnceLock`][__link52] to lazily initialize a [`BytesView`][__link53] from
the static data on first use, using memory from a memory provider that is optimal for the
intended usage.

```rust
use std::sync::OnceLock;

use bytesbuf::BytesView;

const HEADER_PREFIX: &[u8] = b"Unix-Milliseconds: ";

// We transform the static data into a BytesView on first use, via OnceLock.
//
// You are expected to reuse this variable as long as the context does not change.
// For example, it is typically fine to share this across multiple network connections
// because they all likely use the same memory configuration. However, writing to files
// may require a different memory configuration for optimality, so you would need a different
// `BytesView` for that. Such details will typically be documented in the API documentation
// of the type that consumes the `BytesView` (e.g. a network connection or a file writer).
let header_prefix = OnceLock::<BytesView>::new();

for _ in 0..10 {
    let mut connection = Connection::accept();

    // The static data is transformed into a BytesView on first use, using memory optimally configured
    // for network connections. The underlying principle is that memory optimally configured for one network
    // connection is likely also optimally configured for another network connection, enabling efficient reuse.
    let header_prefix = header_prefix
        .get_or_init(|| BytesView::copied_from_slice(HEADER_PREFIX, &connection.memory()));

    // Now we can use the `header_prefix` BytesView in the connection logic.
    // Cloning a BytesView is a cheap zero-copy operation.
    connection.write(header_prefix.clone());
}
```

Different usages (e.g. file vs network) may require differently configured memory for optimal
performance, so you may need a different `BytesView` if the same static data is to be used
in different contexts.

## Testing

For testing purposes, this crate exposes some special-purpose memory providers that are not
optimized for real-world usage but may be useful to test corner cases of byte sequence
processing in your code:

* [`TransparentTestMemory`][__link54] - a memory provider that does not add any value, just uses memory
  from the Rust global allocator.
* [`FixedBlockTestMemory`][__link55] - a variation of the transparent memory provider that limits
  each consecutive memory block to a fixed size. This is useful for testing scenarios where
  you want to ensure that your code works well even if a byte sequence consists of
  non-consecutive memory. You can go down to as low as 1 byte per block!


<hr/>
<sub>
This crate was developed as part of <a href="../..">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/bytesbuf">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGkYW0CYXSEG-dp_C9D4-jCG9tBl5F7M1bDG3mj9J-QdGxQGyUWhRYQiuECYXKEG3iSI3bi0cZkG8Tg7bBQfy7AG_Go-om_ZeJHG0mzQEby4eVxYWSBgmhieXRlc2J1ZmUwLjEuMg
 [__link0]: https://docs.rs/bytesbuf/0.1.2/bytesbuf/?search=BytesBuf
 [__link1]: https://docs.rs/bytesbuf/0.1.2/bytesbuf/?search=BytesView
 [__link10]: https://docs.rs/bytesbuf/0.1.2/bytesbuf/?search=BytesView::first_slice
 [__link11]: https://docs.rs/bytesbuf/0.1.2/bytesbuf/?search=BytesView::first_slice
 [__link12]: https://docs.rs/bytesbuf/0.1.2/bytesbuf/?search=BytesView::first_slice
 [__link13]: https://docs.rs/bytesbuf/0.1.2/bytesbuf/?search=Memory
 [__link14]: https://docs.rs/bytesbuf/0.1.2/bytesbuf/?search=HasMemory
 [__link15]: https://docs.rs/bytesbuf/0.1.2/bytesbuf/?search=HasMemory::memory
 [__link16]: https://docs.rs/bytesbuf/0.1.2/bytesbuf/?search=GlobalPool
 [__link17]: `Memory::reserve()`
 [__link18]: https://docs.rs/bytesbuf/0.1.2/bytesbuf/?search=BytesBuf
 [__link19]: https://docs.rs/bytesbuf/0.1.2/bytesbuf/?search=BytesBuf
 [__link2]: https://docs.rs/bytesbuf/0.1.2/bytesbuf/?search=BytesView
 [__link20]: https://docs.rs/bytesbuf/0.1.2/bytesbuf/?search=BytesBuf
 [__link21]: https://docs.rs/bytesbuf/0.1.2/bytesbuf/?search=BytesBuf
 [__link22]: https://docs.rs/bytesbuf/0.1.2/bytesbuf/?search=BytesBuf::put_num_le
 [__link23]: https://docs.rs/bytesbuf/0.1.2/bytesbuf/?search=BytesBuf::put_slice
 [__link24]: https://docs.rs/bytesbuf/0.1.2/bytesbuf/?search=BytesBuf::put_byte
 [__link25]: https://docs.rs/bytesbuf/0.1.2/bytesbuf/?search=BytesBuf::put_byte_repeated
 [__link26]: https://docs.rs/bytesbuf/0.1.2/bytesbuf/?search=BytesBuf::put_bytes
 [__link27]: https://docs.rs/bytesbuf/0.1.2/bytesbuf/?search=BytesView
 [__link28]: https://docs.rs/bytesbuf/0.1.2/bytesbuf/?search=BytesBuf::first_unfilled_slice
 [__link29]: https://docs.rs/bytesbuf/0.1.2/bytesbuf/?search=BytesBuf::advance
 [__link3]: https://docs.rs/bytesbuf/0.1.2/bytesbuf/?search=BytesView::get_num_le
 [__link30]: https://docs.rs/bytesbuf/0.1.2/bytesbuf/?search=BytesBuf::first_unfilled_slice
 [__link31]: https://docs.rs/bytesbuf/0.1.2/bytesbuf/?search=BytesBuf::first_unfilled_slice
 [__link32]: `BytesBuf::reserve()`
 [__link33]: https://docs.rs/bytesbuf/0.1.2/bytesbuf/?search=BytesBuf::remaining_capacity
 [__link34]: https://docs.rs/bytesbuf/0.1.2/bytesbuf/?search=BytesView
 [__link35]: https://docs.rs/bytesbuf/0.1.2/bytesbuf/?search=BytesView
 [__link36]: https://docs.rs/bytesbuf/0.1.2/bytesbuf/?search=BytesBuf
 [__link37]: `BytesBuf::put_bytes()`
 [__link38]: https://docs.rs/bytesbuf/0.1.2/bytesbuf/?search=HasMemory
 [__link39]: https://docs.rs/bytesbuf/0.1.2/bytesbuf/?search=HasMemory
 [__link4]: https://docs.rs/bytesbuf/0.1.2/bytesbuf/?search=BytesView::get_byte
 [__link40]: https://docs.rs/bytesbuf/0.1.2/bytesbuf/?search=HasMemory
 [__link41]: https://docs.rs/bytesbuf/0.1.2/bytesbuf/?search=HasMemory
 [__link42]: https://docs.rs/bytesbuf/0.1.2/bytesbuf/?search=HasMemory
 [__link43]: https://docs.rs/bytesbuf/0.1.2/bytesbuf/?search=GlobalPool
 [__link44]: https://docs.rs/bytes/latest/bytes/struct.Bytes.html
 [__link45]: https://docs.rs/bytesbuf/0.1.2/bytesbuf/?search=BytesView
 [__link46]: https://docs.rs/bytes/latest/bytes/struct.Bytes.html
 [__link47]: https://docs.rs/bytesbuf/0.1.2/bytesbuf/?search=BytesView::to_bytes
 [__link48]: https://docs.rs/bytesbuf/0.1.2/bytesbuf/?search=BytesView
 [__link49]: https://docs.rs/bytes/latest/bytes/struct.Bytes.html
 [__link5]: https://docs.rs/bytesbuf/0.1.2/bytesbuf/?search=BytesView::copy_to_slice
 [__link50]: https://docs.rs/bytes/latest/bytes/struct.Bytes.html
 [__link51]: https://docs.rs/bytesbuf/0.1.2/bytesbuf/?search=BytesView
 [__link52]: https://doc.rust-lang.org/stable/std/?search=sync::OnceLock
 [__link53]: https://docs.rs/bytesbuf/0.1.2/bytesbuf/?search=BytesView
 [__link54]: https://docs.rs/bytesbuf/0.1.2/bytesbuf/?search=TransparentTestMemory
 [__link55]: https://docs.rs/bytesbuf/0.1.2/bytesbuf/?search=FixedBlockTestMemory
 [__link6]: https://docs.rs/bytesbuf/0.1.2/bytesbuf/?search=BytesView::copy_to_uninit_slice
 [__link7]: https://docs.rs/bytesbuf/0.1.2/bytesbuf/?search=BytesView::as_read
 [__link8]: https://docs.rs/bytesbuf/0.1.2/bytesbuf/?search=BytesView::first_slice
 [__link9]: https://docs.rs/bytesbuf/0.1.2/bytesbuf/?search=BytesView::advance
