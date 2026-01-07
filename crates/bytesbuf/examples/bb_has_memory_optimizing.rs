// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Showcases how to implement the `HasMemory` trait using an optimizing implementation
//! strategy that obtains memory from a memory provider specific to a particular purpose, with
//! a configuration optimal for that purpose.

use bytesbuf::mem::{CallbackMemory, HasMemory, Memory, MemoryShared};
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
    pub const fn new(io_context: IoContext) -> Self {
        Self { io_context }
    }

    #[expect(
        clippy::needless_pass_by_ref_mut,
        clippy::unused_self,
        clippy::needless_pass_by_value,
        reason = "for example realism"
    )]
    pub fn write(&mut self, packet: BytesView) {
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
        CallbackMemory::new({
            // Cloning is cheap, as it is a service that shares resources between clones.
            let io_context = self.io_context.clone();

            move |min_len| io_context.reserve_io_memory(min_len, UDP_CONNECTION_OPTIMAL_MEMORY_CONFIGURATION)
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
    pub const fn new() -> Self {
        Self {}
    }

    #[expect(clippy::unused_self, reason = "for example realism")]
    pub fn reserve_io_memory(&self, min_len: usize, _memory_configuration: MemoryConfiguration) -> BytesBuf {
        // This is a wrong way to implement this! Only to make the example compile.
        let memory = bytesbuf::mem::testing::TransparentMemory::new();
        memory.reserve(min_len)
    }
}

#[expect(dead_code, reason = "just an example, fields unused")]
struct MemoryConfiguration {
    requires_page_alignment: bool,
    zero_memory_on_release: bool,
    requires_registered_memory: bool,
}
