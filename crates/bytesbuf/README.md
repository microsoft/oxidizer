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

Manipulate sequences of bytes for efficient I/O.

A [`BytesView`][__link0] is a view over a logical sequence of zero or more bytes
stored in memory, similar to a slice `&[u8]` but with some key differences:

* The bytes in a byte sequence are not required to be consecutive in memory.
* The bytes in a byte sequence are always immutable, even if you own the [`BytesView`][__link1].

In practical terms, you may think of a byte sequence as a `Vec<Vec<u8>>` whose contents are
treated as one logical sequence of bytes. The types in this crate provide a way to work with
byte sequences using an API that is reasonably convenient while also being compatible with
the requirements of high-performance zero-copy I/O operations.

## Consuming Byte Sequences

The standard model for using bytes of data from a [`BytesView`][__link2] is to consume them via the
[`bytes::buf::Buf`][__link3] trait, which is implemented by [`BytesView`][__link4].

There are many helper methods on this trait that will read bytes from the beginning of the
sequence and simultaneously remove the read bytes from the sequence, shrinking it to only
the remaining bytes.

```rust
use bytes::Buf;
use bytesbuf::BytesView;

fn consume_message(mut message: BytesView) {
    // We read the message and calculate the sum of all the words in it.
    let mut sum: u64 = 0;

    while message.has_remaining() {
        let word = message.get_u64();
        sum = sum.saturating_add(word);
    }

    println!("Message received. The sum of all words in the message is {sum}.");
}
```

If the helper methods are not sufficient, you can access the contents via byte slices using the
more fundamental methods of the [`bytes::buf::Buf`][__link5] trait such as:

* [`chunk()`][__link6], which returns a slice of bytes from the beginning of the sequence. The
  length of this slice is determined by the inner structure of the byte sequence and it may not
  contain all the bytes in the sequence.
* [`advance()`][__link7], which removes bytes from the beginning of the sequence, advancing the
  head to a new position. When you advance past the slice returned by `chunk()`, the next
  call to `chunk()` will return a new slice of bytes starting from the new head position.
* [`chunks_vectored()`][__link8], which returns multiple slices of bytes from the beginning of the
  sequence. This can be desirable for advanced access models that can consume multiple
  chunks of data at the same time.

```rust
use bytes::Buf;
use bytesbuf::BytesView;

let len = sequence.len();
let mut chunk_lengths = Vec::new();

while sequence.has_remaining() {
    let chunk = sequence.chunk();
    chunk_lengths.push(chunk.len());

    // We have completed processing this chunk, all we wanted was to know its length.
    sequence.advance(chunk.len());
}

println!("Inspected a sequence of {len} bytes with chunk lengths: {chunk_lengths:?}");
```

To reuse a byte sequence, clone it before consuming the contents. This is a cheap
zero-copy operation.

```rust
use bytes::Buf;
use bytesbuf::BytesView;

assert_eq!(sequence.len(), 16);

let mut sequence_clone = sequence.clone();
assert_eq!(sequence_clone.len(), 16);

_ = sequence_clone.get_u64();
assert_eq!(sequence_clone.len(), 8);

// Operations on the clone have no effect on the original sequence.
assert_eq!(sequence.len(), 16);
```

## Producing Byte Sequences

For creating a byte sequence, you first need some memory capacity to put the bytes into. This
means you need a memory provider, which is a type that implements the [`Memory`][__link9] trait.

Obtaining a memory provider is generally straightforward. Simply use the first matching option
from the following list:

1. If you are creating byte sequences for the purpose of submitting them to a specific
   object of a known type (e.g. writing them to a network connection), the target type will
   typically implement the [`HasMemory`][__link10] trait, which gives you a suitable memory
   provider instance via [`HasMemory::memory`][__link11]. Use it - this memory provider will
   give you memory with the configuration that is optimal for delivering bytes to that
   specific instance.
1. If you are creating byte sequences as part of usage-neutral data processing, obtain an
   instance of [`GlobalPool`][__link12]. In a typical web application framework, this is a service
   exposed by the application framework. In a different context (e.g. example or test code
   with no framework), you can create your own instance via `GlobalPool::new()`.

Once you have a memory provider, you can reserve memory from it by calling
[`Memory::reserve`][__link13] on it. This returns a [`BytesBuf`][__link14] with the requested
memory capacity.

```rust
use bytesbuf::Memory;

let memory = connection.memory();

let mut sequence_builder = memory.reserve(100);
```

Now that you have the memory capacity and a [`BytesBuf`][__link15], you can fill the memory
capacity with bytes of data. The standard pattern for this is to use the
[`bytes::buf::BufMut`][__link16] trait, which is implemented by [`BytesBuf`][__link17].

Helper methods on this trait allow you to write bytes to the sequence builder up to the
extent of the reserved memory capacity.

```rust
use bytes::buf::BufMut;
use bytesbuf::Memory;

let memory = connection.memory();

let mut sequence_builder = memory.reserve(100);

sequence_builder.put_u64(1234);
sequence_builder.put_u64(5678);
sequence_builder.put(b"Hello, world!".as_slice());
```

If the helper methods are not sufficient, you can append contents via mutable byte slices
using the more fundamental methods of the [`bytes::buf::BufMut`][__link18] trait such as:

* [`chunk_mut()`][__link19], which returns a mutable slice of bytes from the beginning of the
  sequence builder’s unused capacity. The length of this slice is determined by the inner
  structure of the sequence builder and it may not contain all the capacity that has been
  reserved.
* [`advance_mut()`][__link20], which declares that a number of bytes from the beginning of the
  unused capacity have been initialized with data and are no longer unused. This will
  mark these bytes as valid for reading and advance `chunk_mut()` to the next slice if the
  current one has been completely filled.

See `examples/mem_chunk_write.rs` for an example of how to use these methods.

If you do not know exactly how much memory you need in advance, you can extend the sequence
builder capacity on demand if you run out by calling [`BytesBuf::reserve`][__link21],
which will reserve more memory capacity. You can use [`bytes::buf::BufMut::remaining_mut()`][__link22]
on the sequence builder to identify how much unused memory capacity is available for writing.

```rust
use bytes::buf::BufMut;
use bytesbuf::Memory;

let memory = connection.memory();

let mut sequence_builder = memory.reserve(100);

// .. write some data into the sequence builder ..

// We discover that we need 80 additional bytes of memory! No problem.
sequence_builder.reserve(80, &memory);

// Remember that a memory provider can always provide more memory than requested.
assert!(sequence_builder.capacity() >= 100 + 80);
assert!(sequence_builder.remaining_mut() >= 80);
```

When you have filled the memory capacity with the bytes you wanted to write, you can consume
the data in the sequence builder, turning it into a [`BytesView`][__link23] of immutable bytes.

```rust
use bytes::buf::BufMut;
use bytesbuf::Memory;

let memory = connection.memory();

let mut sequence_builder = memory.reserve(100);

sequence_builder.put_u64(1234);
sequence_builder.put_u64(5678);
sequence_builder.put(b"Hello, world!".as_slice());

let message = sequence_builder.consume_all();
```

This can be done piece by piece, and you can continue writing to the sequence builder
after consuming some already written bytes.

```rust
use bytes::buf::BufMut;
use bytesbuf::Memory;

let memory = connection.memory();

let mut sequence_builder = memory.reserve(100);

sequence_builder.put_u64(1234);
sequence_builder.put_u64(5678);

let first_8_bytes = sequence_builder.consume(8);
let second_8_bytes = sequence_builder.consume(8);

sequence_builder.put(b"Hello, world!".as_slice());

let final_contents = sequence_builder.consume_all();
```

If you already have a [`BytesView`][__link24] that you want to write into a [`BytesBuf`][__link25], call
[`BytesBuf::append()`][__link26]. This is a highly efficient zero-copy operation
that reuses the memory capacity of the sequence you are appending.

```rust
use bytes::buf::BufMut;
use bytesbuf::Memory;

let memory = connection.memory();

let mut header_builder = memory.reserve(16);
header_builder.put_u64(1234);
let header = header_builder.consume_all();

let mut sequence_builder = memory.reserve(128);
sequence_builder.append(header);
sequence_builder.put(b"Hello, world!".as_slice());
```

Note that there is no requirement that the memory capacity of the sequence builder and the
memory capacity of the sequence being appended come from the same memory provider. It is valid
to mix and match memory from different providers, though this may disable some optimizations.

## Implementing APIs that Consume Byte Sequences

If you are implementing a type that accepts byte sequences, you should implement the
[`HasMemory`][__link27] trait to make it possible for the caller to use optimally
configured memory.

Even if the implementation of your type today is not capable of taking advantage of
optimizations that depend on the memory configuration, it may be capable of doing so
in the future or may, today or in the future, pass the data to another type that
implements [`HasMemory`][__link28], which can take advantage of memory optimizations.
Therefore, it is best to implement this trait on all types that accept byte sequences.

The recommended implementation strategy for [`HasMemory`][__link29] is as follows:

* If your type always passes the data to another type that implements [`HasMemory`][__link30],
  simply forward the memory provider from the other type.
* If your type can take advantage of optimizations enabled by specific memory configurations,
  (e.g. because it uses operating system APIs that unlock better performance when the memory
  is appropriately configured), return a memory provider that performs the necessary
  configuration.
* If your type neither passes the data to another type that implements [`HasMemory`][__link31]
  nor can take advantage of optimizations enabled by specific memory configurations, obtain
  an instance of [`GlobalPool`][__link32] as a dependency and return it as the memory provider.

Example of forwarding the memory provider (see `examples/mem_has_provider_forwarding.rs`
for full code):

```rust
use bytesbuf::{HasMemory, MemoryShared, BytesView};

/// Counts the number of 0x00 bytes in a sequence before
/// writing that sequence to a network connection.
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

    pub fn write(&mut self, sequence: BytesView) {
        // TODO: Count zeros.

        self.connection.write(sequence);
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
`examples/mem_has_provider_optimizing.rs` for full code):

```rust
use bytesbuf::{CallbackMemory, HasMemory, MemoryShared, BytesView};

/// # Implementation strategy for `HasMemory`
///
/// This type can benefit from optimal performance if specifically configured memory is used and
/// the memory is reserved from the I/O memory pool. It uses the I/O context to reserve memory,
/// providing a usage-specific configuration when reserving memory capacity.
///
/// A delegating memory provider is used to attach the configuration to each memory reservation.
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

Example of returning a usage-neutral memory provider (see `examples/mem_has_provider_neutral.rs` for
full code):

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
    memory_provider: GlobalPool,
}

impl ChecksumCalculator {
    pub fn new(memory_provider: GlobalPool) -> Self {
        Self { memory_provider }
    }
}

impl HasMemory for ChecksumCalculator {
    fn memory(&self) -> impl MemoryShared {
        // Cloning a memory provider is a cheap operation, as clones reuse resources.
        self.memory_provider.clone()
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
`examples/mem_optimal_path.rs` for full code):

```rust
use bytesbuf::BytesView;

pub fn write(&mut self, message: BytesView) {
    // We now need to identify whether the message actually uses memory that allows us to
    // ues the optimal I/O path. There is no requirement that the data passed to us contains
    // only memory with our preferred configuration.

    let use_optimal_path = message.iter_chunk_metas().all(|meta| {
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

The popular [`Bytes`][__link33] type from the `bytes` crate is often used in the Rust ecosystem to
represent simple byte buffers of consecutive bytes. For compatibility with this commonly used
type, this crate offers conversion methods to translate between [`BytesView`][__link34] and [`Bytes`][__link35]:

* [`BytesView::into_bytes`][__link36] converts a [`BytesView`][__link37] into a [`Bytes`][__link38] instance. This
  is not always zero-copy because a byte sequence is not guaranteed to be consecutive in memory.
  You are discouraged from using this method in any performance-relevant logic path.
* `BytesView::from(Bytes)` or `let s: BytesView = bytes.into()` converts a [`Bytes`][__link39] instance
  into a [`BytesView`][__link40]. This is an efficient zero-copy operation that reuses the memory of the
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

The standard pattern here is to use [`OnceLock`][__link41] to lazily initialize a [`BytesView`][__link42] from
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

    // The static data is transformed into a BytesView on first use,
    // using memory optimally configured for a network connection.
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

* [`TransparentTestMemory`][__link43] - a memory provider that does not add any value, just uses memory
  from the Rust global allocator.
* [`FixedBlockTestMemory`][__link44] - a variation of the transparent memory provider that limits
  each consecutive memory block to a fixed size. This is useful for testing scenarios where
  you want to ensure that your code works well even if a byte sequence consists of
  non-consecutive memory. You can go down to as low as 1 byte per block!


<hr/>
<sub>
This crate was developed as part of <a href="../..">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/bytesbuf">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGkYW0CYXSEGy4k8ldDFPOhG2VNeXtD5nnKG6EPY6OfW5wBG8g18NOFNdxpYXKEG5yAxXVX-INyGwACB9YZn6BLG5mUwiQgcGNZGyIa4-qKJXxMYWSBgmhieXRlc2J1ZmUwLjEuMg
 [__link0]: https://docs.rs/bytesbuf/0.1.2/bytesbuf/?search=BytesView
 [__link1]: https://docs.rs/bytesbuf/0.1.2/bytesbuf/?search=BytesView
 [__link10]: https://docs.rs/bytesbuf/0.1.2/bytesbuf/?search=HasMemory
 [__link11]: https://docs.rs/bytesbuf/0.1.2/bytesbuf/?search=HasMemory::memory
 [__link12]: https://docs.rs/bytesbuf/0.1.2/bytesbuf/?search=GlobalPool
 [__link13]: https://docs.rs/bytesbuf/0.1.2/bytesbuf/?search=Memory::reserve
 [__link14]: https://docs.rs/bytesbuf/0.1.2/bytesbuf/?search=BytesBuf
 [__link15]: https://docs.rs/bytesbuf/0.1.2/bytesbuf/?search=BytesBuf
 [__link16]: https://docs.rs/bytes/latest/bytes/buf/trait.BufMut.html
 [__link17]: https://docs.rs/bytesbuf/0.1.2/bytesbuf/?search=BytesBuf
 [__link18]: https://docs.rs/bytes/latest/bytes/buf/trait.BufMut.html
 [__link19]: https://docs.rs/bytes/latest/bytes/buf/trait.BufMut.html#method.chunk_mut
 [__link2]: https://docs.rs/bytesbuf/0.1.2/bytesbuf/?search=BytesView
 [__link20]: https://docs.rs/bytes/latest/bytes/buf/trait.Buf.html#method.advance
 [__link21]: https://docs.rs/bytesbuf/0.1.2/bytesbuf/?search=BytesBuf::reserve
 [__link22]: https://docs.rs/bytes/latest/bytes/buf/trait.BufMut.html#method.remaining_mut
 [__link23]: https://docs.rs/bytesbuf/0.1.2/bytesbuf/?search=BytesView
 [__link24]: https://docs.rs/bytesbuf/0.1.2/bytesbuf/?search=BytesView
 [__link25]: https://docs.rs/bytesbuf/0.1.2/bytesbuf/?search=BytesBuf
 [__link26]: https://docs.rs/bytes/latest/bytes/buf/trait.BufMut.html#method.remaining_mut
 [__link27]: https://docs.rs/bytesbuf/0.1.2/bytesbuf/?search=HasMemory
 [__link28]: https://docs.rs/bytesbuf/0.1.2/bytesbuf/?search=HasMemory
 [__link29]: https://docs.rs/bytesbuf/0.1.2/bytesbuf/?search=HasMemory
 [__link3]: https://docs.rs/bytes/latest/bytes/buf/trait.Buf.html
 [__link30]: https://docs.rs/bytesbuf/0.1.2/bytesbuf/?search=HasMemory
 [__link31]: https://docs.rs/bytesbuf/0.1.2/bytesbuf/?search=HasMemory
 [__link32]: https://docs.rs/bytesbuf/0.1.2/bytesbuf/?search=GlobalPool
 [__link33]: https://docs.rs/bytes/latest/bytes/struct.Bytes.html
 [__link34]: https://docs.rs/bytesbuf/0.1.2/bytesbuf/?search=BytesView
 [__link35]: https://docs.rs/bytes/latest/bytes/struct.Bytes.html
 [__link36]: https://docs.rs/bytesbuf/0.1.2/bytesbuf/?search=BytesView::into_bytes
 [__link37]: https://docs.rs/bytesbuf/0.1.2/bytesbuf/?search=BytesView
 [__link38]: https://docs.rs/bytes/latest/bytes/struct.Bytes.html
 [__link39]: https://docs.rs/bytes/latest/bytes/struct.Bytes.html
 [__link4]: https://docs.rs/bytesbuf/0.1.2/bytesbuf/?search=BytesView
 [__link40]: https://docs.rs/bytesbuf/0.1.2/bytesbuf/?search=BytesView
 [__link41]: https://doc.rust-lang.org/stable/std/?search=sync::OnceLock
 [__link42]: https://docs.rs/bytesbuf/0.1.2/bytesbuf/?search=BytesView
 [__link43]: https://docs.rs/bytesbuf/0.1.2/bytesbuf/?search=TransparentTestMemory
 [__link44]: https://docs.rs/bytesbuf/0.1.2/bytesbuf/?search=FixedBlockTestMemory
 [__link5]: https://docs.rs/bytes/latest/bytes/buf/trait.Buf.html
 [__link6]: https://docs.rs/bytes/latest/bytes/buf/trait.Buf.html#method.chunk
 [__link7]: https://docs.rs/bytes/latest/bytes/buf/trait.Buf.html#method.advance
 [__link8]: https://docs.rs/bytes/latest/bytes/buf/trait.Buf.html#method.chunks_vectored
 [__link9]: https://docs.rs/bytesbuf/0.1.2/bytesbuf/?search=Memory
