// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Showcases how to implement the `HasMemory` trait using an optimizing implementation
//! strategy that obtains memory from a memory provider specific to a particular purpose, with
//! a configuration optimal for that purpose.

use bytesbuf::mem::{HasMemory, Memory, MemoryShared, WrappingMemory};
use bytesbuf::{BytesBuf, BytesView};

fn main() {
    // In a real application, the I/O context would be provided by the framework.
    let io_context = IoContext::new();

    let mut connection = UdpConnection::new(io_context);

    // Prepare a packet to send and send it.
    let mut buf = connection.memory().reserve(1 + 8 + 16);
    buf.put_byte(42);
    buf.put_num_be(43_u64);
    buf.put_num_be(44_u128);

    let packet = buf.consume_all();

    connection.write(packet);
}

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
    pub(crate) const fn new(io_context: IoContext) -> Self {
        Self { io_context }
    }

    #[expect(
        clippy::needless_pass_by_ref_mut,
        clippy::unused_self,
        clippy::needless_pass_by_value,
        reason = "for example realism"
    )]
    pub(crate) fn write(&mut self, packet: BytesView) {
        // Note: making use of optimally configured memory may need some additional logic here.
        // This is out of scope of this example, because this example targets targeting how to
        // implement HasMemory. See `bb_optimal_path.rs` for an example of a type that
        // has both an "optimal" and a "fallback" implementation depending on memory used.
        println!("Sending packet of length: {}", packet.len());
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
        // The wrapped provider carries any thread-affine state and is relocated automatically when
        // this provider moves between threads. The closure captures only inert configuration.
        let io_memory = self.io_context.io_memory();
        let configuration = UDP_CONNECTION_OPTIMAL_MEMORY_CONFIGURATION;

        WrappingMemory::new(io_memory, move |io_memory, min_len| {
            // Apply the connection-specific configuration when reserving from the (relocated)
            // I/O memory provider.
            io_memory.reserve_with_config(min_len, &configuration)
        })
    }
}

// ###########################################################################
// Everything below this comment is dummy logic to make the example compile.
// The useful content of the example is the code above.
// ###########################################################################

#[derive(Clone, Debug)]
struct IoContext;

impl IoContext {
    pub(crate) const fn new() -> Self {
        Self {}
    }

    /// Returns the thread-affine I/O memory provider that reservations are drawn from.
    #[expect(clippy::unused_self, reason = "for example realism")]
    pub(crate) fn io_memory(&self) -> IoMemory {
        IoMemory
    }
}

/// The thread-affine I/O memory provider. In a real application this would carry per-thread I/O
/// resources; here it is a thin wrapper for illustration.
#[derive(Clone, Debug)]
struct IoMemory;

impl IoMemory {
    #[expect(clippy::unused_self, reason = "for example realism")]
    fn reserve_with_config(&self, min_len: usize, _memory_configuration: &MemoryConfiguration) -> BytesBuf {
        // This is a wrong way to implement this! Only to make the example compile.
        let memory = bytesbuf::mem::testing::TransparentMemory::new();
        memory.reserve(min_len)
    }
}

impl Memory for IoMemory {
    fn reserve(&self, min_bytes: usize) -> BytesBuf {
        self.reserve_with_config(
            min_bytes,
            &MemoryConfiguration {
                requires_page_alignment: false,
                zero_memory_on_release: false,
                requires_registered_memory: false,
            },
        )
    }
}

impl thread_aware::ThreadAware for IoMemory {
    fn relocate(&mut self, _source: Option<thread_aware::affinity::Affinity>, _destination: thread_aware::affinity::Affinity) {
        // A real provider would relocate its per-thread I/O resources here.
    }
}

#[expect(dead_code, reason = "just an example, fields unused")]
struct MemoryConfiguration {
    requires_page_alignment: bool,
    zero_memory_on_release: bool,
    requires_registered_memory: bool,
}
